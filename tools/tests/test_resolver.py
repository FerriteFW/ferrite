"""ferrite_link.resolve_port (#217): ports, radio names, the default, FERRITE_PORT and the CH340 search.

    python3 -m unittest discover -s tools/tests"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import ferrite_link as fl  # noqa: E402

RADIOS = {
    "default": "black",
    "radios": {
        "black": {"cable": True, "ble": "AAAA-1111", "build": "tx-bench"},
        "clear": {"ble": "BBBB-2222", "build": "tx"},
        "bench": {"cable": "/dev/ttyUSB7"},
        "lost": {"note": "no links"},
    },
}
ONE = ["/dev/cu.usbserial-1420"]
TWO = ["/dev/cu.usbserial-1420", "/dev/cu.usbserial-1430"]


def resolve(spec=None, found=(), env=None, radios=RADIOS, **kw):
    return fl.resolve_port(spec, env=env or {}, radios=radios, glob_fn=lambda _: list(found), platform="darwin", **kw)


class Explicit(unittest.TestCase):
    def test_ports_pass_through(self):
        for spec in ("/dev/cu.usbserial-9", "ble:walkie-talkie", "tcp:127.0.0.1:7300", "COM3"):
            self.assertEqual(resolve(spec, TWO), spec)

    def test_cable_only_refuses_ble_and_tcp(self):
        with self.assertRaises(fl.PortError):
            resolve("ble:X", cable_only=True)


class Names(unittest.TestCase):
    def test_cable_when_exactly_one(self):
        self.assertEqual(resolve("black", ONE), ONE[0])

    def test_ble_when_no_cable_or_several(self):
        self.assertEqual(resolve("black", []), "ble:AAAA-1111")
        self.assertEqual(resolve("black", TWO), "ble:AAAA-1111")

    def test_ble_suffix_forces_bluetooth(self):
        self.assertEqual(resolve("black:ble", ONE), "ble:AAAA-1111")

    def test_cable_suffix(self):
        self.assertEqual(resolve("black:cable", ONE), ONE[0])
        with self.assertRaises(fl.PortError):
            resolve("black:cable", TWO)
        with self.assertRaises(fl.PortError):
            resolve("clear:cable", ONE)

    def test_ble_only_radio(self):
        self.assertEqual(resolve("clear", ONE), "ble:BBBB-2222")

    def test_fixed_device(self):
        self.assertEqual(resolve("bench", [], cable_only=True), "/dev/ttyUSB7")

    def test_cable_only_never_ble(self):
        self.assertEqual(resolve("black", ONE, cable_only=True), ONE[0])
        with self.assertRaises(fl.PortError):
            resolve("black", [], cable_only=True)
        with self.assertRaises(fl.PortError):
            resolve("black:ble", ONE, cable_only=True)

    def test_unknown_name_lists_radios(self):
        with self.assertRaises(fl.PortError) as e:
            resolve("purple", ONE)
        self.assertIn("black", str(e.exception))

    def test_radio_without_links(self):
        with self.assertRaises(fl.PortError):
            resolve("lost", ONE)


class Nothing(unittest.TestCase):
    def test_env_first(self):
        self.assertEqual(resolve(None, ONE, env={"FERRITE_PORT": "tcp:127.0.0.1:7300"}), "tcp:127.0.0.1:7300")
        self.assertEqual(resolve("auto", ONE, env={"FERRITE_PORT": "clear"}), "ble:BBBB-2222")

    def test_default(self):
        self.assertEqual(resolve(None, []), "ble:AAAA-1111")
        self.assertEqual(resolve("", ONE), ONE[0])

    def test_only_cable(self):
        self.assertEqual(resolve(None, ONE, radios={}), ONE[0])

    def test_none_or_several(self):
        with self.assertRaises(fl.PortError) as e:
            resolve(None, [], radios={})
        self.assertIn("no cable", str(e.exception))
        with self.assertRaises(fl.PortError) as e:
            resolve(None, TWO, radios={})
        self.assertIn(TWO[1], str(e.exception))

    def test_linux_glob(self):
        got = fl.resolve_port(None, env={}, radios={}, glob_fn=lambda pat: ["/dev/ttyUSB0"] if "ttyUSB" in pat else [],
                              platform="linux")
        self.assertEqual(got, "/dev/ttyUSB0")


class File(unittest.TestCase):
    def test_reads_toml(self):
        with tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False) as f:
            f.write('default = "x"\n[radios.x]\nble = "CCCC"\n')
        try:
            self.assertEqual(fl.load_radios(f.name)["radios"]["x"]["ble"], "CCCC")
            self.assertEqual(fl.load_radios(f.name + ".missing"), {})
        finally:
            os.unlink(f.name)

    def test_example_parses(self):
        ex = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "radios.example.toml")
        radios = fl.load_radios(ex)
        self.assertIn(radios["default"], radios["radios"])


if __name__ == "__main__":
    unittest.main()
