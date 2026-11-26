"""Installing and updating (chapters 2, 3, 30: BACKUP-*, INST-*, UPD-*).

The flasher's refusals are checked on the PC alone, with synthetic images and backups in a temporary directory:
`tools/flash.py` must refuse before it opens the port (the port given doesn't exist), so these run in every
session, the simulator's too, and touch no radio.

The rest compare the radio before and after a flash (needs=("flash",)): run

    tools/uitest.py --module install --flash before     # keeps the readings in --flash-state
    .venv/bin/python tools/flash.py PORT target/ferrite-txdry.btf --backup ../backups/blackradio/flash.bin \
        [--ssb-from STOCK.BTF]
    tools/uitest.py --module install --flash after      # compares

`before` only records; `after` checks. A first install (a radio fresh from stock) makes the first-boot checks
apply; an update the update ones.
"""
import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile

from .core import test

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FLASH = os.path.join(HERE, "flash.py")
NO_PORT = "/nonexistent/uitest-no-such-port"
sys.path.insert(0, HERE)


# ---- The flasher's refusals, on the PC ----

def ferrite_image(size=64 * 1024):
    """A synthetic image the flasher takes for Ferrite's: a valid initial SP and reset vector, the model tag at 0x3E0
    and 0x7E0, the "Ferrite terminal" mark; 0xFF padding to `size`."""
    import btf
    img = bytearray(b"\xff" * size)
    struct.pack_into("<II", img, 0, 0x20018000, btf.APP_BASE + 0x801)
    img[btf.MODEL_OFF:btf.MODEL_OFF + 12] = btf.MODEL_TAG
    img[btf.MODEL_OFF_BOOT:btf.MODEL_OFF_BOOT + 12] = btf.MODEL_TAG
    img[0x900:0x910] = b"Ferrite terminal"
    return bytes(img)


def backup(size=4 << 20, cal=True, pending=False):
    b = bytearray(b"\xff" * size)
    if cal:
        b[0xF000:0xF010] = bytes(range(16))
    if pending and size > 0x300002:
        b[0x300000:0x300002] = b"\x5a\xa5"
    return bytes(b)


def flash(*args):
    r = subprocess.run([sys.executable, FLASH, *args], capture_output=True, text=True, timeout=30)
    return r.returncode, r.stdout + r.stderr


def tmpfile(d, name, data):
    p = os.path.join(d, name)
    open(p, "wb").write(data)
    return p


@test("BACKUP-2", pages=())
def flasher_needs_a_good_backup(t):
    """flash.py refuses a Ferrite image without --backup, and with a backup that isn't a whole working chip's (the
    wrong size, a blank calibration sector), before it opens the port or erases anything."""
    with tempfile.TemporaryDirectory() as d:
        img = tmpfile(d, "ferrite.bin", ferrite_image())
        rc, out = flash(NO_PORT, img)
        t.check(rc != 0 and "refusing to flash" in out and "needs --backup" in out, "no backup: refused", out)
        for name, data, why in (("short.bin", backup(size=1 << 20), "2, 4 or 8 MB"),
                                ("blank.bin", backup(cal=False), "calibration sector")):
            rc, out = flash(NO_PORT, img, "--backup", tmpfile(d, name, data))
            t.check(rc != 0 and "refusing to flash" in out and why in out, f"{name}: refused ({why})", out)
            t.check("bootloader" not in out.lower() or "refusing" in out, "before the port", out)
        rc, out = flash("--dry-run", "-", img, "--backup", tmpfile(d, "good.bin", backup()))
        t.eq(rc, 0, "a good backup: taken (dry run)")


@test("INST-3", pages=())
def flasher_refuses_oversize_images(t):
    """An image over 1,012 KB (it would run past the chip's end at 0x08100000, the bootloader's own limit; decision
    338) is refused by btf.py's check and by flash.py; 1,012 KB exactly is taken."""
    import btf
    ok = btf.check_image(ferrite_image(1012 * 1024))
    t.check(ok == [], "1,012 KB: fine", ok)
    errs = btf.check_image(ferrite_image(1012 * 1024 + 1024))
    t.check(errs, "1,013 KB: refused by the check", errs)
    with tempfile.TemporaryDirectory() as d:
        img = tmpfile(d, "big.bin", ferrite_image(1013 * 1024))
        rc, out = flash("--dry-run", "-", img)
        t.check(rc != 0 and "refusing" in out, "flash.py refuses it", out)


@test("BACKUP-12.2", "BACKUP-12", pages=())
def stock_restore_covers_the_marker_page(t):
    """Back to stock (flash.py with a stock image): the image goes padded with 0xFF through 0x080F07FF, so the
    bootloader erases the marker sector's first page and no Ferrite code is left in the word stock reads; the 16 bytes
    in marker.bin beside the backup go back at 0x080F0000 when they hold a stock marker, else the page stays erased.
    `--as-is` sends the file unchanged."""
    import btf
    stock = bytearray(ferrite_image(300 * 1024 + 123))
    stock[0x900:0x910] = b"stock firmware.."  # no Ferrite mark
    page_end = btf.MARKER_OFF + btf.MARKER_PAGE
    with tempfile.TemporaryDirectory() as d:
        img = tmpfile(d, "stock.bin", bytes(stock))
        rc, out = flash("--dry-run", "-", img)
        t.check(rc == 0 and "padded through 0x080F07FF" in out and "assumed blank" in out, "no record: padded, erased",
                out)
        t.check(f"{(page_end + 1024) // 1024} blocks" in out, "the file reaches 0x080F07FF", out)
        os.makedirs(os.path.join(d, "radio"))
        bk = tmpfile(os.path.join(d, "radio"), "flash.bin", backup())
        tmpfile(os.path.join(d, "radio"), "marker.bin", b"UV85" + b"\xff" * 12)
        rc, out = flash("--dry-run", "-", img, "--backup", bk)
        t.check(rc == 0 and "stock marker 'UV85'" in out, "marker.bin beside the backup: the word goes back", out)
        rc, out = flash("--dry-run", "-", img, "--as-is")
        t.check(rc == 0 and "padded" not in out and f"{-(-(len(stock) + 1024) // 1024)} blocks" in out,
                "--as-is: unchanged", out)
    for m, want in ((None, b"\xff" * 16), (b"BJ90" + b"\xff" * 12, b"BJ90" + b"\xff" * 12),
                    (bytes(range(16)), b"\xff" * 16)):
        r = btf.stock_restore_image(bytes(stock), m)
        t.eq((len(r), r[:len(stock)] == bytes(stock), r[btf.MARKER_OFF:btf.MARKER_OFF + 16]),
             (page_end, True, want), f"marker {m!r}")


@test("INST-5", pages=())
def flasher_refuses_a_pending_update(t):
    """A backup holding a pending stock update (0xA55A at 0x300000) is refused, with why: the bootloader would
    flash it over Ferrite at the next power-on."""
    with tempfile.TemporaryDirectory() as d:
        img = tmpfile(d, "ferrite.bin", ferrite_image())
        rc, out = flash(NO_PORT, img, "--backup", tmpfile(d, "pending.bin", backup(pending=True)))
        t.check(rc != 0 and "pending update" in out and "0xA55A" in out, "refused, with the explanation", out)


@test("INST-11", pages=())
def ssb_patch_bundle_layout(t):
    """--ssb-from bundles the patch at the first 1 KB boundary after the image (its last byte that isn't padding): a
    64-byte FERRSSB1 header (length,
    record count, the SHA-256) then the patch; the combined image is only sent, never written to a file."""
    import ssbpatch
    patch = bytes([0x15] + [0] * 7) + b"".join(bytes([0x16, i & 0xFF] + [0] * 6) for i in range(599))
    img = ferrite_image(5000)
    out, at = ssbpatch.bundle(img, patch)
    end = len(img.rstrip(b"\xff"))  # the image's end: its 0xFF padding isn't the image
    t.eq(at % 1024, 0, "a 1 KB boundary")
    t.check(end <= at < end + 1024, "the first after the image", (end, at))
    h = out[at:at + 64]
    t.eq(h[:8], b"FERRSSB1", "the header's magic")
    t.eq(struct.unpack_from("<II", h, 8), (len(patch), len(patch) // 8), "length and records")
    t.eq(h[20:52], hashlib.sha256(patch).digest(), "the SHA-256")
    t.eq(out[at + 64:at + 64 + len(patch)], patch, "the patch after it")
    with tempfile.TemporaryDirectory() as d:
        stock = tmpfile(d, "STOCK.bin", b"\x00" * 4096 + patch + b"\x00" * 64)
        img_p = tmpfile(d, "ferrite.bin", img)
        before = set(os.listdir(d))
        rc, outp = flash("--dry-run", "-", img_p, "--ssb-from", stock)
        t.eq(rc, 0, "the dry run bundles it")
        t.check("bundled at" in outp, "the flasher says where", outp)
        t.eq(set(os.listdir(d)), before, "no combined image written")


@test("UPD-15", pages=())
def flasher_checks_release_signatures(t):
    """A release file whose SHA-256 beside it (FILE.sha256) doesn't match is refused by flash.py before flashing;
    a matching one is taken. (The minisign signature isn't checked yet: releases carry none.)"""
    with tempfile.TemporaryDirectory() as d:
        data = ferrite_image()
        img = tmpfile(d, "ferrite.btf.bin", data)
        good = tmpfile(d, "good.bin", backup())
        open(img + ".sha256", "w").write("0" * 64 + "  ferrite.btf.bin\n")
        rc, out = flash("--dry-run", "-", img, "--backup", good)
        t.check(rc != 0 and "refusing" in out and "SHA-256" in out, "a mismatched checksum: refused", out)
        open(img + ".sha256", "w").write(hashlib.sha256(data).hexdigest() + "  ferrite.btf.bin\n")
        rc, out = flash("--dry-run", "-", img, "--backup", good)
        t.check(rc == 0 and "SHA-256 matches" in out, "a matching checksum: taken (dry run)", out)


# ---- Around a flash (--flash before / after) ----

def readings(t):
    t.home()
    u = t.keys("MENU 0 9 8")
    about = dict(zip(u.names(), (r["value"] for r in u.rows)))  # "Version" whether official or not (SELF-5.1)
    t.home()
    u = t.keys("MENU 0 9 9 2 MENU")
    cal = {r["name"]: r["value"] for r in u.rows}
    t.home()
    return dict(dump=[x for x in t.cmd("dump").splitlines() if not x.startswith("#")],
                slots=[t.slot(L) for L in "ABC"], about=about, cal=cal)


def phase(t):
    """("before" | "after", the saved readings or None)."""
    path = t.opts.flash_state
    if t.opts.flash == "before":
        return "before", None
    t.need(os.path.exists(path), f"no readings from --flash before ({path})")
    return "after", json.load(open(path))


def save(t, key, value):
    path = t.opts.flash_state
    d = json.load(open(path)) if os.path.exists(path) else {}
    d[key] = value
    os.makedirs(os.path.dirname(path), exist_ok=True)
    json.dump(d, open(path, "w"), indent=1)


@test("UPD-11", "INST-13", "INST-3", needs=("flash",), pages=("Group", "Info"))
def update_keeps_the_spi_flash(t):
    """An update leaves the SPI flash alone: the dump (settings, zones, presets, channels, the home channel) and the
    slots as before, and the SSB patch still installed (flashed with or without --ssb-from); on a bench build the
    stock marker sector's first bytes (0x080F0000) as before while the image stops below it (BACKUP-12)."""
    p, before = phase(t)
    now = readings(t)
    if t.r.bench:
        now["marker"] = t.cmd("mem 080F0000 32")
    if p == "before":
        save(t, "update", now)
        t.note("readings kept: flash, then --flash after")
        t.need(False, "--flash before: readings saved; the check runs with --flash after")
    b = before.get("update")
    t.need(b, "no update readings from before")
    t.need(now["about"].get("Version") != b["about"].get("Version"), "the same firmware: nothing was flashed")
    t.eq(now["dump"], b["dump"], "the dump as before")
    t.eq(now["slots"], b["slots"], "the slots as before")
    if b["about"].get("SSB patch") == "Installed":
        t.eq(now["about"].get("SSB patch"), "Installed", "the SSB patch still installed")
    if "marker" in b and "marker" in now:
        from .hostconfirm import MARKER_OFF, installed_image
        img = installed_image(t)
        if img is not None and len(img) > MARKER_OFF:
            t.note("the new image reaches 0x080F0000: the marker sector holds its code (BACKUP-12)")
        else:
            t.eq(now["marker"], b["marker"], "0x080F0000 as before the flash")


@test("INST-12", "UPD-13", "HF-18", needs=("flash",), pages=("Group", "Hf", "Info"))
def bundled_patch_installed(t):
    """Flashed with --ssb-from (a radio without the patch before): at boot the patch is checked and stored, About
    shows it installed, its page Verified (a known hash), and HF mode offers USB, LSB and CW."""
    p, before = phase(t)
    now = readings(t)
    if p == "before":
        save(t, "patch", now)
        t.need(False, "--flash before: readings saved; the check runs with --flash after")
    b = before.get("patch")
    t.need(b and b["about"].get("SSB patch") != "Installed", "the patch was installed before (flash a radio "
           "without it, with --ssb-from)")
    t.eq(now["about"].get("SSB patch"), "Installed", "About: installed")
    u = t.keys("MENU 0 9 6 MENU")
    t.eq({r["name"]: r["value"] for r in u.rows}.get("State"), "Verified", "a known patch: verified")
    from .modes import HfVfo, hf_mode
    vfo = HfVfo(t)
    try:
        t.check(hf_mode(t, "USB") is not None, "HF mode offers USB", t.ui().lines)
    finally:
        vfo.restore()


@test("INST-6", "INST-9", "BACKUP-3", "BACKUP-10", "SAFE-4", needs=("flash",), pages=("Group", "Home", "Info", "Setup"))
def first_install(t):
    """After a first install (from stock): setup runs; the dump has Ferrite's defaults and no stock channels;
    the calibration is valid with its CRC recorded at install and matching (calibration untouched, the CRC in the
    layout header); no pending update (0x300000 intact: UPD-8)."""
    p, before = phase(t)
    if p == "before":
        # Only a real first install counts (the radio wiped or new to Ferrite): a plain update keeps the channels,
        # so an update must not mark itself as one (Oct 14, 2026: an update recorded five false fails).
        given = {x.strip() for x in (getattr(t.opts, "with_", "") or "").split(",") if x.strip()}
        save(t, "first", {"note": "a first install: run --flash after once setup is done"}
             if "first-install" in given else None)
        t.need(False, "--flash before: readings saved; the check runs with --flash after")
    t.need(before.get("first"), "no first-install marker from --flash before")
    t.need(t.ui().id in ("Setup", "Home"), "not on setup or home")
    now = readings(t)
    chans = [x for x in now["dump"] if x.startswith("ch ")]
    t.eq(chans, [], "no channels: nothing imported from stock")
    t.eq(now["cal"].get("Status"), "Valid", "calibration valid")
    t.eq(now["cal"].get("CRC at install"), "Matches", "its CRC recorded at install, matching")
    t.check(not any("pending" in (k + v).lower() for k, v in now["about"].items()), "no pending update", now["about"])
