#!/usr/bin/env python3
"""Host tests for the update file (decision 335; UPD-20, UPD-21, UPD-24): `python3 -m unittest tools/test_ffw.py`.

Builds `.ffw` files with a throwaway key and has the installers' logic (tools/ffw.py, which flash.py uses) take a
good one and refuse an unsigned one without the developer flag, another key's, a changed byte, and parts aimed at
calibration, the update area, the layout header or outside the parts' regions, whatever the manifest says. The
part writer runs against a fake radio.
"""
import io
import json
import os
import struct
import sys
import tempfile
import unittest
import zipfile
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ffw  # noqa: E402

SEED = bytes(range(32))
BUILD = "abc1234"


def part(magic, body, build=BUILD):
    return struct.pack("<4sHH16sII", magic, 1, 0, build.encode(), len(body), zlib.crc32(body)) + body


def make(parts=None, sign=True, build=BUILD, edit=None, extra=None):
    """A .ffw's bytes and the test key's public key text. `edit(manifest)` changes the manifest before signing;
    `extra` adds files after."""
    btf = bytes(range(256)) * 40
    if parts is None:
        parts = [("text-en.bin", part(b"FTXT", b"\x01\x00\x00\x00\x08\x00\x00\x02hi"), 0x090000, 0x10000),
                 ("logo.bin", part(b"FLGO", b"\x00" * 100), 0x240000, 0x8000)]
    m = ffw.make_manifest(btf, parts, "tx", build, "0.9.0")
    if edit:
        edit(m)
    data = json.dumps(m).encode()
    sig, pub = ffw.minisign_sig(SEED, data)
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as z:
        z.writestr("manifest.json", data)
        if sign:
            z.writestr("manifest.minisig", sig)
        z.writestr("firmware.btf", btf)
        for n, b, _, _ in parts:
            z.writestr(f"spi/{n}", b)
        for n, b in (extra or {}).items():
            z.writestr(n, b)
    return out.getvalue(), pub


def problems(b, pub, dev=False):
    try:
        ffw.check(b, dev_unsigned=dev, pubkey_text=pub)
        return []
    except ffw.Refused as e:
        return e.problems


class Check(unittest.TestCase):
    def test_a_signed_file_installs(self):
        b, pub = make()
        m, entries, signed = ffw.check(b, pubkey_text=pub)
        self.assertTrue(signed)
        self.assertEqual([(n, a) for n, a, _ in ffw.spi_parts(m, entries)], [("spi/text-en.bin", 0x090000), ("spi/logo.bin", 0x240000)])

    def test_the_pack_tool_makes_a_file_check_takes(self):
        with tempfile.TemporaryDirectory() as d:
            open(os.path.join(d, "x.btf"), "wb").write(b"\0" * 2048)
            open(os.path.join(d, "text-en.bin"), "wb").write(part(b"FTXT", b"body"))
            json.dump([{"file": "text-en.bin", "magic": "FTXT", "addr": 0x090000, "max": 0x10000}], open(os.path.join(d, "parts.json"), "w"))
            ffw.pack(os.path.join(d, "x.ffw"), os.path.join(d, "x.btf"), d, "tx", BUILD, "0.9.0")
            with zipfile.ZipFile(os.path.join(d, "x.ffw")) as z:
                first = z.infolist()[0]
                self.assertEqual((first.filename, first.compress_type), ("manifest.json", zipfile.ZIP_STORED))
            self.assertIn("unsigned", " ".join(problems(open(os.path.join(d, "x.ffw"), "rb").read(), None)))
            m, _, signed = ffw.check(open(os.path.join(d, "x.ffw"), "rb"), dev_unsigned=True)
            self.assertFalse(signed)
            # the signature goes in second, as CI puts it
            sig, pub = ffw.minisign_sig(SEED, ffw.read(open(os.path.join(d, "x.ffw"), "rb"))[0])
            ffw.add_sig(os.path.join(d, "x.ffw"), sig)
            self.assertTrue(ffw.check(open(os.path.join(d, "x.ffw"), "rb"), pubkey_text=pub)[2])

    def test_unsigned_needs_the_dev_flag(self):
        b, pub = make(sign=False)
        self.assertIn("unsigned", problems(b, pub)[0])
        self.assertEqual(problems(b, pub, dev=True), [])

    def test_another_key_is_refused(self):
        b, _ = make()
        _, other = ffw.minisign_sig(bytes(32), b"x")
        self.assertTrue(any("signature" in p for p in problems(b, other)))
        # the release key: a test key's signature is never taken for it
        self.assertTrue(problems(b, open(ffw.PUBKEY).read()))

    def test_a_changed_byte_is_refused(self):
        b, pub = make()
        z = zipfile.ZipFile(io.BytesIO(b))
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as w:
            for i in z.infolist():
                d = z.read(i)
                if i.filename == "firmware.btf":
                    d = d[:100] + bytes([d[100] ^ 1]) + d[101:]
                w.writestr(i.filename, d)
        self.assertTrue(any("SHA-256" in p for p in problems(out.getvalue(), pub)))
        # a damaged zip
        self.assertTrue(problems(b.replace(b"\x10\x11\x12", b"\x10\x11\x13", 1), pub))
        # the manifest changed after signing: the signature fails
        out = io.BytesIO()
        with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as w:
            for i in z.infolist():
                d = z.read(i)
                if i.filename == "manifest.json":
                    d = d.replace(b'"version": "0.9.0"', b'"version": "0.9.1"')
                w.writestr(i.filename, d)
        self.assertTrue(any("signature" in p for p in problems(out.getvalue(), pub)))

    def test_parts_aimed_at_protected_ranges_are_refused_even_signed(self):
        for addr, most, what in [(0x00F000, 0x1000, "calibration"), (0x00E000, 0x1000, "migration mark"),
                                 (0x00E000, 0x2000, "calibration"), (0x000000, 0x1000, "layout header"),
                                 (0x300000, 0x1000, "update area"), (0x2FF000, 0x2000, "update area"),
                                 (0x010000, 0x1000, "outside"), (0x2C0000, 0x1000, "outside"),
                                 (0x091800, 0x1000, "sector")]:
            def edit(m, addr=addr, most=most):
                m["files"][1]["addr"] = addr
                m["files"][1]["max"] = most
            b, pub = make(edit=edit)
            p = problems(b, pub)
            self.assertTrue(any(what in x for x in p), f"0x{addr:06X}: {p}")

    def test_a_part_past_its_maximum_is_refused(self):
        def edit(m):
            m["files"][1]["max"] = 8
        b, pub = make(edit=edit)
        self.assertTrue(any("more than" in p for p in problems(b, pub)))

    def test_a_part_of_another_build_is_refused(self):
        parts = [("text-en.bin", part(b"FTXT", b"x", build="zzz9999"), 0x090000, 0x10000)]
        b, pub = make(parts=parts)
        self.assertTrue(any("doesn't name this build" in p for p in problems(b, pub)))

    def test_unlisted_files_and_other_formats_are_refused(self):
        b, pub = make(extra={"spi/evil.bin": b"x"})
        self.assertTrue(any("doesn't list" in p for p in problems(b, pub)))

        def edit(m):
            m["format"] = "ferrite-update-9"
        b, pub = make(edit=edit)
        self.assertTrue(problems(b, pub))
        self.assertTrue(problems(b"not a zip", None))


class FakeRadio:
    """Ferrite's side of DATA_READ/DATA_WRITE (LINK-59, LINK-60) over a 4 MB chip, `busy` now and then."""

    def __init__(self, layouts=(3, 3)):
        self.chip = bytearray(b"\xff" * (4 << 20))
        self.ops, self.n, self.stamped = [], 0, False
        # protocol 1's HELLO answer: protocol, max payload, may go on, version, build, caps, layouts, lists generation
        self.hello = bytes([1, 240, 1, 7]) + b"Ferrite" + bytes([1, 0]) + bytes(layouts) + bytes(4)

    def ask(self, ty, p):
        self.n += 1
        if self.n % 7 == 0:
            return ffw.BUSY, b""
        if ty == ffw.DATA_WRITE and p[0] == 0:
            a = struct.unpack("<I", p[1:5])[0]
            assert not ffw.spi_target_problem("x", a, 4096, 4096) or 0x090000 <= a < 0x2C0000
            self.chip[a:a + 4096] = b"\xff" * 4096
            self.ops.append(("erase", a))
        elif ty == ffw.DATA_WRITE and p[0] == 1:
            a, d = struct.unpack("<I", p[1:5])[0], p[5:]
            assert len(d) <= 64 and a // 256 == (a + len(d) - 1) // 256
            self.chip[a:a + len(d)] = bytes(x & y for x, y in zip(self.chip[a:a + len(d)], d))
            self.ops.append(("program", a))
        elif ty == ffw.DATA_WRITE and p[0] == 2:
            self.stamped = True
        elif ty == ffw.DATA_READ and p[0] == 0:
            a, n = struct.unpack("<I", p[1:5])[0], p[5]
            return ffw.OK, bytes(self.chip[a:a + n])
        elif ty == ffw.DATA_READ and p[0] == 1:
            a, n = struct.unpack("<II", p[1:9])
            return ffw.OK, struct.pack("<I", zlib.crc32(bytes(self.chip[a:a + n])))
        return ffw.OK, b""


class Clock:
    t = 0.0

    def time(self):
        return self.t

    def sleep(self, s):
        self.t += s


class Write(unittest.TestCase):
    def test_parts_are_written_read_back_and_stamped(self):
        b, pub = make()
        m, entries, _ = ffw.check(b, pubkey_text=pub)
        r = FakeRadio()
        ffw.write_parts(r.ask, r.hello, ffw.spi_parts(m, entries), say=lambda s: None, clock=Clock())
        for n, a, d in ffw.spi_parts(m, entries):
            self.assertEqual(bytes(r.chip[a:a + len(d)]), d, n)
        self.assertTrue(r.stamped)
        self.assertEqual({a for op, a in r.ops if op == "erase"}, {0x090000, 0x240000})

    def test_the_writer_refuses_protected_ranges_itself(self):
        r = FakeRadio()
        for addr in (0x00F000, 0x300000, 0x000000, 0x00E000):
            with self.assertRaises(ffw.Refused):
                ffw.write_parts(r.ask, r.hello, [("spi/x.bin", addr, b"\0" * 10)], say=lambda s: None, clock=Clock())
        self.assertEqual(r.ops, [])

    def test_a_radio_waiting_for_its_data_move_gets_nothing(self):
        b, pub = make()
        m, entries, _ = ffw.check(b, pubkey_text=pub)
        r = FakeRadio(layouts=(3, 2))
        with self.assertRaises(ffw.Refused):
            ffw.write_parts(r.ask, r.hello, ffw.spi_parts(m, entries), say=lambda s: None, clock=Clock())
        self.assertEqual(r.ops, [])

    def mark(self, r, frm):
        rec = struct.pack("<IHHI", ffw.MARK_MAGIC, frm, 0, 0)
        r.chip[0x00E000:0x00E010] = rec + struct.pack("<I", zlib.crc32(rec))

    def test_parts_cut_before_their_stamp_are_written_again(self):
        """UPD-16: 255 with a mark that began from this layout: only the parts were cut; writing them finishes it."""
        b, pub = make()
        m, entries, _ = ffw.check(b, pubkey_text=pub)
        r = FakeRadio(layouts=(3, 255))
        self.mark(r, 3)
        ffw.write_parts(r.ask, r.hello, ffw.spi_parts(m, entries), say=lambda s: None, clock=Clock())
        self.assertTrue(r.stamped)

    def test_a_half_moved_radio_gets_nothing(self):
        """UPD-16: 255 with no mark, or a mark from another layout (a real move cut): Console's copy finishes it."""
        b, pub = make()
        m, entries, _ = ffw.check(b, pubkey_text=pub)
        for frm in (None, 2):
            r = FakeRadio(layouts=(3, 255))
            if frm is not None:
                self.mark(r, frm)
            with self.assertRaises(ffw.Refused):
                ffw.write_parts(r.ask, r.hello, ffw.spi_parts(m, entries), say=lambda s: None, clock=Clock())
            self.assertEqual([op for op in r.ops if op[0] != "read"], [])


if __name__ == "__main__":
    unittest.main()
