#!/usr/bin/env python3
"""Ferrite's update file, `.ffw` (decision 335; manual UPD-20 - UPD-24): one file per update.

    python3 tools/ffw.py pack OUT.ffw --btf target/ferrite-tx.btf --parts target/parts --kind tx
    python3 tools/ffw.py check FILE.ffw [--dev-unsigned]   # the installers' checks, nothing written
    python3 tools/ffw.py manifest FILE.ffw > manifest.json  # the bytes the release key signs
    python3 tools/ffw.py add-sig FILE.ffw manifest.minisig  # put the signature in (CI)
    python3 tools/ffw.py build-id IMAGE.bin                 # the build ID a firmware image carries

The file is a zip. Its first entry, stored uncompressed, is `manifest.json` (format `ferrite-update-1`): the build
ID, version, model, build kind, the SPI flash layout it needs, and every file with its size, SHA-256 and target
(`mcu` for `firmware.btf`; `spi` with the address and the most it may take for each `spi/` part). Then
`manifest.minisig` when signed (the release key's minisign signature over the manifest's bytes), `firmware.btf`
(the MCU image as today: another flasher can still take it out) and the `spi/` parts. Every entry is stored, so any
zip reader, a browser's included, opens it without inflating.

Installing (`flash.py`, Console): the signature and every hash are checked first; an unsigned file only with an
explicit developer flag (`--dev-unsigned`). Then the backup as today, the MCU flashed, then the SPI parts over the
link's raw write (LINK-60). A part aimed at calibration (0x00F000-0x00FFFF), the update area (0x300000 up), the
layout header or the migration mark, or outside the layout 3 region parts may use, is refused whatever the manifest
says (UPD-24).
"""
import argparse
import base64
import hashlib
import io
import json
import os
import struct
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FORMAT = "ferrite-update-1"
MODEL = "RT-950"          # the model tag the bootloader checks (btf.MODEL_OFF)
LAYOUT = 3                # crates/ferrite-core/src/layout.rs LAYOUT_VERSION
PUBKEY = os.path.join(ROOT, "minisign.pub")
BTF_MAX = 0xFD000 + 0x400  # the bootloader's image limit, plus the BTF's own header block
SECTOR = 0x1000

# UPD-24: never written, whatever a manifest says (layout.rs: CALIBRATION, UPDATE_AREA, LAYOUT_HEADER, MIGRATE_MARK).
CALIBRATION = (0x00F000, 0x010000)
UPDATE_AREA = (0x300000, 0x400000)
LAYOUT_HEADER = (0x000000, 0x001000)
MIGRATE_MARK = (0x00E000, 0x00F000)
FORBIDDEN = [("calibration", CALIBRATION), ("the update area", UPDATE_AREA), ("the layout header", LAYOUT_HEADER),
             ("the migration mark", MIGRATE_MARK)]
# The layout 3 regions SPI parts may go in (layout.rs): the text pack, the dictionary, language packs, boot screens,
# fonts.
PART_REGIONS = [("text pack", (0x090000, 0x0A0000)), ("dictionary", (0x0B0000, 0x0C0000)), ("language packs", (0x210000, 0x240000)),
                ("boot screens", (0x240000, 0x280000)), ("fonts", (0x280000, 0x2C0000))]
PART_HEADER = 32
PART_FORMAT = 1


class Refused(Exception):
    """The file is not one to install: the problems, one a line."""

    def __init__(self, problems):
        super().__init__("\n".join(problems))
        self.problems = problems


# ---- the safety rule (UPD-24) -----------------------------------------------------------------------------------
def spi_target_problem(name, addr, size, most):
    """Why a part may not be written there (None: it may). Checked against fixed ranges, never the manifest's."""
    if not isinstance(addr, int) or not isinstance(size, int) or not isinstance(most, int) or addr < 0 or size <= 0:
        return f"{name}: no valid address and size"
    if size > most:
        return f"{name}: {size} bytes, more than its {most}"
    end = addr + max(size, most)
    for what, (a, b) in FORBIDDEN:
        if addr < b and end > a:
            return f"{name}: aimed at {what} (0x{addr:06X}-0x{end - 1:06X}): refused whatever the manifest says"
    if addr % SECTOR:
        return f"{name}: 0x{addr:06X} is not on a 4 KB sector"
    for _, (a, b) in PART_REGIONS:
        if a <= addr and end <= b:
            return None
    return f"{name}: 0x{addr:06X}-0x{end - 1:06X} is outside the layout 3 regions parts may use"


# ---- minisign (Ed25519; pure Python, RFC 8032's reference) ---------------------------------------------------------
_P = 2 ** 255 - 19
_L = 2 ** 252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)


def _add(a, b):
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    A, B = (y1 - x1) * (y2 - x2) % _P, (y1 + x1) * (y2 + x2) % _P
    C, Dd = t1 * 2 * _D * t2 % _P, z1 * 2 * z2 % _P
    E, F, G, H = B - A, Dd - C, Dd + C, B + A
    return E * F % _P, G * H % _P, F * G % _P, E * H % _P


def _mul(s, pt):
    q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            q = _add(q, pt)
        pt = _add(pt, pt)
        s >>= 1
    return q


def _equal(a, b):
    x1, y1, z1, _ = a
    x2, y2, z2, _ = b
    return (x1 * z2 - x2 * z1) % _P == 0 and (y1 * z2 - y2 * z1) % _P == 0


def _recover_x(y, sign):
    if y >= _P:
        return None
    x2 = (y * y - 1) * pow(_D * y * y + 1, _P - 2, _P)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P:
        x = x * _SQRT_M1 % _P
    if (x * x - x2) % _P:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


_GY = 4 * pow(5, _P - 2, _P) % _P
_G = (_recover_x(_GY, 0), _GY, 1, _recover_x(_GY, 0) * _GY % _P)


def _decompress(s):
    if len(s) != 32:
        return None
    y = int.from_bytes(s, "little")
    sign, y = y >> 255, y & ((1 << 255) - 1)
    x = _recover_x(y, sign)
    return None if x is None else (x, y, 1, x * y % _P)


def _compress(pt):
    x, y, z, _ = pt
    zi = pow(z, _P - 2, _P)
    x, y = x * zi % _P, y * zi % _P
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def ed25519_verify(public, msg, sig):
    if len(public) != 32 or len(sig) != 64:
        return False
    a, r = _decompress(public), _decompress(sig[:32])
    s = int.from_bytes(sig[32:], "little")
    if a is None or r is None or s >= _L:
        return False
    h = int.from_bytes(hashlib.sha512(sig[:32] + public + msg).digest(), "little") % _L
    return _equal(_mul(s, _G), _add(r, _mul(h, a)))


def ed25519_sign(seed, msg):
    """For the tests' throwaway key only (releases are signed with minisign itself)."""
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    public = _compress(_mul(a, _G))
    r = int.from_bytes(hashlib.sha512(h[32:] + msg).digest(), "little") % _L
    rs = _compress(_mul(r, _G))
    k = int.from_bytes(hashlib.sha512(rs + public + msg).digest(), "little") % _L
    return rs + int.to_bytes((r + k * a) % _L, 32, "little"), public


def read_pubkey(text):
    """A minisign public key (the file's text or its base64 line): (key ID, Ed25519 key)."""
    lines = [ln.strip() for ln in text.strip().splitlines() if ln.strip() and not ln.startswith("untrusted comment:")]
    raw = base64.b64decode(lines[-1])
    if len(raw) != 42 or raw[:2] != b"Ed":
        raise ValueError("not a minisign public key")
    return raw[2:10], raw[10:]


def minisign_verify(pubkey_text, data, sig_text):
    """Problems with a minisign signature over `data` (empty: good)."""
    key_id, pk = read_pubkey(pubkey_text)
    lines = sig_text.decode("utf-8", "replace").splitlines() if isinstance(sig_text, bytes) else sig_text.splitlines()
    if len(lines) < 4 or not lines[0].startswith("untrusted comment:") or not lines[2].startswith("trusted comment: "):
        return ["manifest.minisig is not a minisign signature"]
    try:
        raw, glob = base64.b64decode(lines[1]), base64.b64decode(lines[3])
    except ValueError:
        return ["manifest.minisig is not a minisign signature"]
    if len(raw) != 74 or len(glob) != 64:
        return ["manifest.minisig is not a minisign signature"]
    alg, kid, sig = raw[:2], raw[2:10], raw[10:]
    if kid != key_id:
        return [f"signed with key {kid[::-1].hex().upper()}, not the release key {key_id[::-1].hex().upper()}"]
    if alg == b"ED":
        msg = hashlib.blake2b(data, digest_size=64).digest()
    elif alg == b"Ed":
        msg = data
    else:
        return ["manifest.minisig: unknown signature algorithm"]
    if not ed25519_verify(pk, msg, sig):
        return ["the signature doesn't match the manifest: a changed or damaged file"]
    if not ed25519_verify(pk, sig + lines[2][len("trusted comment: "):].encode(), glob):
        return ["the signature's trusted comment was changed"]
    return []


def minisign_sig(seed, data, comment="test"):
    """A minisign signature (prehashed) with a throwaway key, and its public key's text: the tests'."""
    key_id = b"\x01\x02\x03\x04\x05\x06\x07\x08"
    sig, pk = ed25519_sign(seed, hashlib.blake2b(data, digest_size=64).digest())
    glob, _ = ed25519_sign(seed, sig + comment.encode())
    text = (f"untrusted comment: signature from a test key\n{base64.b64encode(b'ED' + key_id + sig).decode()}\n"
            f"trusted comment: {comment}\n{base64.b64encode(glob).decode()}\n")
    pub = f"untrusted comment: test key\n{base64.b64encode(b'Ed' + key_id + pk).decode()}\n"
    return text.encode(), pub


# ---- the file ----------------------------------------------------------------------------------------------------
def sha256(b):
    return hashlib.sha256(b).hexdigest()


def part_header(b):
    """An SPI part's header (ferrite_core::part): (magic, format, build ID, body length, CRC), or None."""
    if len(b) < PART_HEADER:
        return None
    magic, fmt, _, build, n, crc = struct.unpack("<4sHH16sII", b[:PART_HEADER])
    return magic.decode("latin-1"), fmt, build.rstrip(b"\0").decode("latin-1"), n, crc


def make_manifest(btf, parts, kind, build, version, model=MODEL, layout=LAYOUT):
    """The manifest for an image and its parts ([(name, bytes, addr, most)])."""
    files = [{"file": "firmware.btf", "target": "mcu", "size": len(btf), "sha256": sha256(btf), "max": BTF_MAX}]
    for name, b, addr, most in parts:
        files.append({"file": f"spi/{name}", "target": "spi", "addr": addr, "max": most, "size": len(b),
                      "sha256": sha256(b)})
    return {"format": FORMAT, "build": build, "version": version, "model": model, "kind": kind, "layout": layout,
            "files": files}


def write(path, manifest, contents, minisig=None):
    """The zip: manifest.json first and stored, then the signature, firmware.btf and the parts (all stored)."""
    data = json.dumps(manifest, indent=1).encode() + b"\n"
    tmp = path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
        z.writestr(zipfile.ZipInfo("manifest.json", (2026, 1, 1, 0, 0, 0)), data)
        if minisig is not None:
            z.writestr(zipfile.ZipInfo("manifest.minisig", (2026, 1, 1, 0, 0, 0)), minisig)
        for f in manifest["files"]:
            z.writestr(zipfile.ZipInfo(f["file"], (2026, 1, 1, 0, 0, 0)), contents[f["file"]])
    os.replace(tmp, path)
    return data


def pack(path, btf_path, parts_dir, kind, build, version):
    def slurp(path):
        with open(path, "rb") as f:
            return f.read()
    btf = slurp(btf_path)
    parts = []
    if parts_dir:
        for p in json.loads(slurp(os.path.join(parts_dir, "parts.json"))):
            parts.append((p["file"], slurp(os.path.join(parts_dir, p["file"])), p["addr"], p["max"]))
    m = make_manifest(btf, parts, kind, build, version)
    contents = {"firmware.btf": btf, **{f"spi/{n}": b for n, b, _, _ in parts}}
    write(path, m, contents)
    return m


def read(path_or_bytes):
    """The manifest's bytes, the signature (or None) and every entry's bytes; problems with the container raise."""
    src = io.BytesIO(path_or_bytes) if isinstance(path_or_bytes, (bytes, bytearray)) else path_or_bytes
    try:
        z = zipfile.ZipFile(src)
    except zipfile.BadZipFile:
        raise Refused(["not a .ffw file (not a zip)"])
    infos = z.infolist()
    if not infos or infos[0].filename != "manifest.json" or infos[0].compress_type != zipfile.ZIP_STORED:
        raise Refused(["not a .ffw file: its first entry must be manifest.json, stored"])
    try:
        entries = {i.filename: z.read(i) for i in infos}
    except (zipfile.BadZipFile, ValueError, NotImplementedError, OSError) as e:
        raise Refused([f"a damaged .ffw file ({e})"])
    return entries["manifest.json"], entries.get("manifest.minisig"), entries


def check(path_or_bytes, dev_unsigned=False, pubkey_text=None):
    """Everything the installers check before writing anything (UPD-21, UPD-24): the manifest, the signature, every
    file's size and SHA-256, the parts' headers and targets. Returns (manifest, entries, signed); raises Refused."""
    mbytes, sig, entries = read(path_or_bytes)
    problems = []
    try:
        m = json.loads(mbytes)
    except ValueError:
        raise Refused(["manifest.json isn't JSON"])
    if m.get("format") != FORMAT:
        raise Refused([f"format {m.get('format')!r}: this installer reads {FORMAT}"])
    if sig is None:
        if not dev_unsigned:
            problems.append("unsigned: only a release (signed with the release key) installs; a local build needs "
                            "the developer flag (--dev-unsigned)")
    else:
        if pubkey_text is None:
            pubkey_text = open(PUBKEY).read()
        problems += minisign_verify(pubkey_text, mbytes, sig)
    if m.get("model") != MODEL:
        problems.append(f"for model {m.get('model')!r}, not this radio ({MODEL})")
    files = m.get("files") or []
    if not any(f.get("file") == "firmware.btf" and f.get("target") == "mcu" for f in files):
        problems.append("no firmware.btf in the manifest")
    listed = {"manifest.json", "manifest.minisig"}
    for f in files:
        name = f.get("file", "?")
        listed.add(name)
        b = entries.get(name)
        if b is None:
            problems.append(f"{name}: in the manifest, not in the file")
            continue
        if len(b) != f.get("size") or sha256(b) != f.get("sha256"):
            problems.append(f"{name}: its size or SHA-256 doesn't match the manifest: a changed or damaged file")
            continue
        if f.get("target") == "mcu":
            if name != "firmware.btf" or len(b) > f.get("max", 0) or len(b) > BTF_MAX:
                problems.append(f"{name}: not an MCU image this bootloader takes")
        elif f.get("target") == "spi":
            why = spi_target_problem(name, f.get("addr"), len(b), f.get("max", 0))
            if why:
                problems.append(why)
                continue
            h = part_header(b)
            if h is None or h[1] != PART_FORMAT or h[2] != m.get("build") or h[3] + PART_HEADER != len(b):
                problems.append(f"{name}: its header doesn't name this build ({m.get('build')})")
        else:
            problems.append(f"{name}: unknown target {f.get('target')!r}")
    extra = sorted(set(entries) - listed)
    if extra:
        problems.append(f"files the manifest doesn't list: {', '.join(extra)}")
    if problems:
        raise Refused(problems)
    return m, entries, sig is not None


def spi_parts(m, entries):
    """The SPI parts to write, in the manifest's order: (name, addr, bytes). Each target checked again here."""
    out = []
    for f in m["files"]:
        if f["target"] == "spi":
            b = entries[f["file"]]
            why = spi_target_problem(f["file"], f["addr"], len(b), f["max"])
            if why:
                raise Refused([why])
            out.append((f["file"], f["addr"], b))
    return out


# ---- writing the parts over the link (LINK-59 - LINK-62) ---------------------------------------------------------
DATA_READ, DATA_WRITE = 0x61, 0x62
OK, BUSY = 0, 7
PROGRAM_MAX = 64
def hello_layouts(hello):
    """LINK-62: (the layout the firmware stores, the one the chip's data is in), from HELLO's answer: after the
    version text, the build and the capability byte (protocol 1, decision 385)."""
    if len(hello) < 4:
        return (0, 0)
    t = hello[4 + hello[3]:]
    return (t[2], t[3]) if len(t) >= 4 else (0, 0)


def write_parts(ask, hello, parts, say=print, busy_s=15.0, clock=None):
    """Writes the SPI parts to a running Ferrite over the cable (UPD-21): each part's sectors erased, its bytes
    programmed, its CRC read back; then the layout stamp (LINK-62), on which the radio restarts. `ask(type, payload)`
    returns (status, payload); `hello` is HELLO's answer. Every target is checked again first (UPD-24)."""
    import time as _t
    import zlib
    clock = clock or _t
    for name, addr, b in parts:
        why = spi_target_problem(name, addr, len(b), len(b))
        if why:
            raise Refused([why])
    this, found = hello_layouts(hello)
    if this != LAYOUT or found != LAYOUT:
        raise Refused([f"the radio's firmware stores layout {this} and its data is in layout {found}: finish the update "
                       "with Ferrite Console (it moves the data), then write the parts again (flash.py --parts-only)"])

    def go(ty, payload):
        end = clock.time() + busy_s
        while True:
            st, rest = ask(ty, payload)
            if st == OK:
                return rest
            if st != BUSY or clock.time() > end:
                raise Refused([f"the radio answered {st} to 0x{ty:02X}: nothing more written; run --parts-only again"])
            clock.sleep(0.02)

    for k, (name, addr, b) in enumerate(parts, 1):
        ask(DATA_WRITE, struct.pack("<BBB", 3, k, len(parts)))  # UPD-16.2: "Part 2 of 4" on the radio; any answer
        for sec in range(addr, addr + len(b), SECTOR):
            go(DATA_WRITE, struct.pack("<BI", 0, sec))
        for off in range(0, len(b), PROGRAM_MAX):
            chunk = b[off:off + PROGRAM_MAX]
            if chunk.count(0xFF) != len(chunk):                   # erased bytes need no program
                go(DATA_WRITE, struct.pack("<BI", 1, addr + off) + chunk)
        got = struct.unpack("<I", go(DATA_READ, struct.pack("<BII", 1, addr, len(b))))[0]
        if got != zlib.crc32(b):
            raise Refused([f"{name}: read back with CRC {got:08X}, written {zlib.crc32(b):08X}"])
        say(f"{name}: {len(b)} bytes at 0x{addr:06X}, read back")
    go(DATA_WRITE, struct.pack("<BH", 2, LAYOUT))
    say("stamped: the radio restarts with its parts")


def build_id(image):
    """The build ID (SELF-5) a firmware image carries: the commit, with "+" when built from changes."""
    import subprocess
    sha = os.environ.get("FERRITE_COMMIT") or subprocess.run(["git", "rev-parse", "--short=7", "HEAD"], capture_output=True, text=True, cwd=ROOT).stdout.strip()
    for cand in (sha + "+", sha):
        if cand and cand.encode() in image:
            return cand
    return None


def add_sig(path, minisig):
    _, _, entries = read(open(path, "rb"))
    m = json.loads(entries["manifest.json"])
    mbytes = entries["manifest.json"]
    tmp = path + ".tmp"
    with zipfile.ZipFile(tmp, "w", zipfile.ZIP_STORED) as z:
        z.writestr(zipfile.ZipInfo("manifest.json", (2026, 1, 1, 0, 0, 0)), mbytes)
        z.writestr(zipfile.ZipInfo("manifest.minisig", (2026, 1, 1, 0, 0, 0)), minisig)
        for f in m["files"]:
            z.writestr(zipfile.ZipInfo(f["file"], (2026, 1, 1, 0, 0, 0)), entries[f["file"]])
    os.replace(tmp, path)


def describe(m, signed):
    parts = ", ".join(f"{f['file'][4:]} {f['size']} B at 0x{f['addr']:06X}" for f in m["files"] if f["target"] == "spi")
    fw = next(f for f in m["files"] if f["target"] == "mcu")
    return (f"Ferrite {m['version']} build {m['build']} ({m['kind']}, layout {m['layout']}), "
            f"{'signed' if signed else 'UNSIGNED'}: firmware.btf {fw['size']} B; {parts or 'no SPI parts'}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("pack")
    p.add_argument("out")
    p.add_argument("--btf", required=True)
    p.add_argument("--parts", help="a directory with parts.json and the parts (ferrite-mkparts)")
    p.add_argument("--kind", required=True)
    p.add_argument("--build", required=True)
    p.add_argument("--version", required=True)
    c = sub.add_parser("check")
    c.add_argument("file")
    c.add_argument("--dev-unsigned", action="store_true", help="accept an unsigned file (a local build)")
    sub.add_parser("manifest").add_argument("file")
    s = sub.add_parser("add-sig")
    s.add_argument("file")
    s.add_argument("minisig")
    b = sub.add_parser("build-id")
    b.add_argument("image")
    a = ap.parse_args()
    if a.cmd == "pack":
        m = pack(a.out, a.btf, a.parts, a.kind, a.build, a.version)
        print(f"{a.out}: {describe(m, False)}")
    elif a.cmd == "check":
        try:
            m, _, signed = check(open(a.file, "rb"), a.dev_unsigned)
        except Refused as e:
            sys.exit("refused:\n  " + "\n  ".join(e.problems))
        print(describe(m, signed))
    elif a.cmd == "manifest":
        sys.stdout.buffer.write(read(open(a.file, "rb"))[0])
    elif a.cmd == "add-sig":
        add_sig(a.file, open(a.minisig, "rb").read())
    elif a.cmd == "build-id":
        bid = build_id(open(a.image, "rb").read())
        if not bid:
            sys.exit(f"{a.image}: no build ID for this commit in the image (built from another commit? rebuild)")
        print(bid)


if __name__ == "__main__":
    main()
