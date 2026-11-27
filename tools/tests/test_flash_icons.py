"""flash.py's menu-icons line for a stock image (BACKUP-12.3): an ICON firmware's own tiles, an icons file, or none.

    python3 -m unittest discover -s tools/tests"""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
import btf  # noqa: E402
import flash  # noqa: E402

# Tiles as stock's look to icons_plausible: runs of the same pixel along a row.
TILES = bytes([0x12, 0x34] * (btf.ICONS_LEN // 2))


class IconsNote(unittest.TestCase):
    def test_an_icon_firmware_carries_its_own(self):
        img = bytes(btf.ICONS_OFF) + TILES + bytes(4096)
        self.assertEqual(flash.icons_note(img, None, None), "in the image itself (an ICON firmware)")

    def test_a_short_image_takes_the_backups(self):
        self.assertEqual(flash.icons_note(bytes(300_000), TILES, "b.fbin"), "from b.fbin")

    def test_a_short_image_without_icons_has_none(self):
        self.assertIn("none: the tiles stay blank", flash.icons_note(bytes(300_000), None, None))

    def test_code_where_the_tiles_go_is_not_tiles(self):
        code = bytes((i * 37 + i // 3) & 0xFF for i in range(btf.ICONS_LEN))
        img = bytes(btf.ICONS_OFF) + code
        self.assertIn("none", flash.icons_note(img, None, None))


if __name__ == "__main__":
    unittest.main()
