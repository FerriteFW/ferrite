#!/usr/bin/env python3
"""Flash an image into the RT-950 Pro over the programming cable, through the stock bootloader.

    pip install pyserial
    python3 tools/flash.py /dev/tty.usbserial-XXXX build/target/hwtest.btf
    python3 tools/flash.py /dev/tty.usbserial-XXXX build/target/hwtest.bin     # packed on the fly
    python3 tools/flash.py --dry-run - stock.BTF                               # checks only, no radio
    python3 tools/flash.py PORT target/ferrite.btf --backup flash.bin         # Ferrite: needs the backup
    python3 tools/flash.py auto target/ferrite-tx.ffw ...                     # PORT: auto, or a radio's name
    python3 tools/flash.py PORT stock.BTF --backup flash.bin                  # back to stock, marker put back
    python3 tools/flash.py PORT ferrite-0.9.ffw --backup flash.bin           # an update file (decision 335)
    python3 tools/flash.py PORT target/ferrite-tx.ffw --backup flash.bin --dev-unsigned   # a local build's
    python3 tools/flash.py PORT FILE.ffw --parts-only                         # only the SPI parts, radio running

A `.ffw` (tools/ffw.py, UPD-20 - UPD-24) is checked whole before anything is written: the release key's signature
(an unsigned file only with --dev-unsigned), every file's SHA-256, each SPI part's target (never calibration, the
update area, the layout header or the migration mark, nor outside the layout 3 regions parts may use, whatever the
manifest says). Then the MCU is flashed as below; once Ferrite is running, its SPI parts are written over the link's
raw write (LINK-60, the cable only), each read back by CRC, and the layout stamped (LINK-62): the radio restarts.

Ferrite images (they contain "Ferrite terminal") are refused without --backup: the whole SPI flash read with
the hwtest image (tools/flashdump.py, in ../fw). The backup must look like this radio's chip (2, 4 or 8 MB,
calibration present at 0xF000), and it must not hold a pending phone/Bluetooth update (magic 0xA55A at
0x300000): the bootloader would flash that over Ferrite at the next power-on (roadmap 1b). Ferrite never
writes the update area, not even to clear it, so restore stock and update it the normal way first.

The MCU's top 64 KB (0x080F0000, stock's marker sector: decision 338, BACKUP-12) holds one word only stock firmware
writes (UV85 or BJ90, the model name on stock's info page); a Ferrite image past 948 KB overwrites it. Its first 16
bytes are recorded at backup time in `marker.bin` beside the backup (fw/tools/backup.py, read with the reader's or
hwtest's `mdump 0x080F0000 16`; or `--marker FILE`). A stock image (neither Ferrite, the reader nor the restorer) goes padded with
0xFF through 0x080F07FF, the bootloader erasing that page, and the recorded word put back when it is a stock marker;
without a record the page is left erased (stock then shows its default model name). `--as-is` sends a file unchanged.

Getting into the bootloader needs nothing from the running firmware: at every power-on the
bootloader listens about 200 ms for a probe packet before starting the app. This script floods
probes and, to make the radio restart, sends Ferrite's link REBOOT (LINK-24), `reboot` (hwtest's console)
and the stock PROGRAMBT9000U / UPDATE sequence. If neither works, power-cycle the radio while it waits.
Holding the bottom two side buttons at power-on (inputs PA12 and PE5) also enters the bootloader.

Protocol (from the bootloader code; ../analysis/bootloader.md): frames are
AA cmd seq(2) len(2) data CRC16-XMODEM(cmd..data, big-endian) 55. Replies are 9 bytes,
AA cmd 00 result 00 00 crc crc 55, result 06 = ACK.
  0A hello, 02 model tag, 04 block count, 03 one 1 KB block (seq = file block), 45 end and reset.
The block count is (file blocks - 1), as Radtel's updater sends it: the bootloader then decrypts
everything but the last block, which .BTF files leave plain.
"""
import argparse
import json
import os
import struct
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import btf  # noqa: E402

CMD_HELLO, CMD_MODEL, CMD_COUNT, CMD_DATA, CMD_END, CMD_PROBE = 0x0A, 0x02, 0x04, 0x03, 0x45, 0x42
ACK = 0x06
RESULTS = {0x06: "ok", 0xE1: "bad frame or length", 0xE2: "block out of order or too far",
           0xE3: "flash write failed", 0xE5: "command not valid now", 0xE6: "model tag rejected"}


def crc16(data):
    c = 0
    for x in data:
        c ^= x << 8
        for _ in range(8):
            c = ((c << 1) ^ 0x1021) & 0xFFFF if c & 0x8000 else (c << 1) & 0xFFFF
    return c


def frame(cmd, seq=0, data=b""):
    body = bytes([cmd]) + struct.pack(">HH", seq, len(data)) + data
    return b"\xAA" + body + struct.pack(">H", crc16(body)) + b"\x55"


def read_reply(port, timeout):
    """Next valid 9-byte reply as (cmd, result), or None."""
    buf, end = b"", time.monotonic() + timeout
    while time.monotonic() < end:
        buf += port.read(port.in_waiting or 1)
        while len(buf) >= 9:
            i = buf.find(b"\xAA")
            if i < 0:
                buf = b""
                break
            buf = buf[i:]
            if len(buf) < 9:
                break
            f = buf[:9]
            if f[8] == 0x55 and crc16(f[1:6]) == struct.unpack(">H", f[6:8])[0]:
                return f[1], f[3]
            buf = buf[1:]
    return None


def ferrite_reboot():
    """Ferrite's link REBOOT request (manual Appendix G, LINK-24): a 0x00 to claim the cable for the link, then
    COBS(type 0x21, seq 1, CRC-16/CCITT-FALSE little-endian) and its 0x00."""
    body = bytes([0x21, 1])
    c = 0xFFFF
    for x in body:
        c ^= x << 8
        for _ in range(8):
            c = (c << 1) ^ 0x1021 if c & 0x8000 else c << 1
            c &= 0xFFFF
    raw = body + struct.pack("<H", c)
    out, block = bytearray(), bytearray()
    for x in raw:                                           # COBS; no frame here reaches 254 bytes
        if x == 0:
            out += bytes([len(block) + 1]) + block
            block = bytearray()
        else:
            block.append(x)
    out += bytes([len(block) + 1]) + block
    return b"\x00" + bytes(out) + b"\x00"


def enter_bootloader(port, wait):
    probe = frame(CMD_PROBE)
    port.reset_input_buffer()
    port.write(ferrite_reboot())                            # Ferrite: saves, goes quiet, resets
    time.sleep(0.1)
    port.write(b"\r\nreboot\r\n")                         # hwtest
    t0, nudged = time.monotonic(), False
    print(f"waiting up to {wait} s for the bootloader (power-cycle the radio if it does not restart)...")
    while time.monotonic() - t0 < wait:
        if not nudged and time.monotonic() - t0 > 1.5:      # still running stock firmware?
            nudged = True
            port.write(b"PROGRAMBT9000U")
            time.sleep(0.3)
            if ACK in port.read(port.in_waiting or 0):
                port.write(b"UPDATE")
                time.sleep(0.1)
        port.write(probe)
        r = read_reply(port, 0.012)
        if r and r[0] == CMD_PROBE:                           # 06 in the boot window, E5 once inside
            time.sleep(0.1)
            port.reset_input_buffer()
            return True
    return False


def command(port, cmd, seq=0, data=b"", timeout=2.0, tries=3):
    for _ in range(tries):
        port.reset_input_buffer()
        port.write(frame(cmd, seq, data))
        r = read_reply(port, timeout)
        if r is None or r[1] == 0xE1:                         # lost or garbled: the same frame again
            continue
        return r[1]
    return None


FERRITE_MARK = b"Ferrite terminal"
CAL, SECTOR, UPDATE_AREA = 0xF000, 0x1000, 0x300000


def read_backup(path):
    """A backup's parts: (SPI flash, marker 16 bytes or None, menu icons or None). A .fbin (decision 407: a zip of
    manifest.json, spi.bin and icons.bin, each checked by the manifest's SHA-256) or a plain whole-chip .bin."""
    import hashlib
    import zipfile
    raw = open(path, "rb").read()
    if not raw.startswith(b"PK\x03\x04"):
        return raw, None, None
    with zipfile.ZipFile(path) as z:
        m = json.loads(z.read("manifest.json"))
        if m.get("format") != "ferrite-backup":
            sys.exit(f"{path}: not a Ferrite backup (.fbin)")
        spi = z.read("spi.bin")
        if hashlib.sha256(spi).hexdigest() != m["spi"]["sha256"]:
            sys.exit(f"{path}: its spi.bin doesn't match its manifest (a damaged file)")
        icons = z.read("icons.bin") if "icons.bin" in z.namelist() else None
        if icons is not None and (not m.get("icons") or hashlib.sha256(icons).hexdigest() != m["icons"]["sha256"]):
            sys.exit(f"{path}: its icons.bin doesn't match its manifest (a damaged file)")
    marker = bytes.fromhex(m["marker"]) if m.get("marker") else None
    return spi, marker, icons


def check_backup(path):
    """Problems with a whole-chip backup (empty list: fine), and a line describing it."""
    import hashlib
    import zlib
    try:
        b = read_backup(path)[0]
    except OSError as e:
        return [f"can't read the backup: {e}"], ""
    errs = []
    if len(b) not in (2 << 20, 4 << 20, 8 << 20):
        errs.append(f"{path} is {len(b)} bytes: a whole-chip backup is 2, 4 or 8 MB (flashdump.py without --cal)")
    cal = b[CAL:CAL + SECTOR]
    if len(cal) < SECTOR or cal.count(0xFF) == SECTOR or cal.count(0) == SECTOR:
        errs.append("the calibration sector (0xF000) in the backup is blank: not a backup of a working radio")
    hdr = b[UPDATE_AREA:UPDATE_AREA + 2]
    if hdr in (b"\x5a\xa5", b"\xa5\x5a"):
        errs.append("the backup holds a pending update (0xA55A at 0x300000): the bootloader would flash it over "
                    "Ferrite at the next power-on. Restore stock, let it finish or clear that update the stock "
                    "way, take a new backup, then install")
    desc = (f"backup {path}: {len(b) >> 20} MB, sha256 {hashlib.sha256(b).hexdigest()[:16]}, "
            f"calibration crc32 {zlib.crc32(cal):08X}")
    return errs, desc


READER_MARK = b"fdump <addr> <len>"
RESTORER_MARK = b"Ferrite restorer: BACK13"  # restorer/src/main.rs (BACKUP-13): sent as it is, as the reader


def load_marker(path):
    """The marker sector's 16 bytes recorded at backup time, or None when there is no record."""
    if not path or not os.path.isfile(path):
        return None
    m = open(path, "rb").read()
    if len(m) != btf.MARKER_LEN:
        sys.exit(f"{path} is {len(m)} bytes: a marker record is the {btf.MARKER_LEN} bytes at 0x080F0000")
    return m


def marker_path(a):
    """--marker, else marker.bin beside the backup."""
    if a.marker:
        return a.marker
    return os.path.join(os.path.dirname(os.path.abspath(a.backup)), "marker.bin") if a.backup else None


def check_sha256(path, data):
    """UPD-15: a release file's SHA-256 beside it (FILE.sha256, `sha256sum` format) must match the file; no
    checksum file, nothing to check. (The minisign signature isn't checked yet: releases carry none.)"""
    import hashlib
    side = path + ".sha256"
    if not os.path.exists(side):
        return []
    want = (open(side).read().split() or [""])[0].lower()
    got = hashlib.sha256(data).hexdigest()
    if want != got:
        return [f"{os.path.basename(path)}: its SHA-256 is {got[:16]}..., but {os.path.basename(side)} says "
                f"{want[:16]}...: a corrupted or tampered download"]
    print(f"{os.path.basename(path)}: SHA-256 matches {os.path.basename(side)}")
    return []


def icons_note(img, icons, ipath):
    """What the menu tiles will be after a stock image (BACKUP-12.3): the image's own when it covers them with tiles
    (Radtel's ICON firmware), else the icons file or backup's, else none (Nov 26, 2026: an ICON firmware was
    reported as "none")."""
    tiles = img[btf.ICONS_OFF:btf.ICONS_OFF + btf.ICONS_LEN]
    if len(img) >= btf.ICONS_OFF + btf.ICONS_LEN and btf.icons_plausible(tiles):
        return "in the image itself (an ICON firmware)"
    return f"from {ipath}" if icons else "none: the tiles stay blank (BACKUP-12.3)"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("port", help="the programming cable: a device, a radio's name (radios.toml) or auto ('-' with --dry-run)")
    ap.add_argument("file", help=".btf to send, or a raw .bin to pack first")
    ap.add_argument("--wait", type=float, default=30, help="seconds to wait for the bootloader")
    ap.add_argument("--dry-run", action="store_true", help="check the file and stop")
    ap.add_argument("--progress", action="store_true", help="show a progress line per block (off: quiet until done)")
    ap.add_argument("--backup", metavar="FLASH.BIN",
                    help="the whole-SPI-flash backup taken with the hwtest image (required for Ferrite images)")
    ap.add_argument("--ssb-from", metavar="STOCK.BTF",
                    help="bundle the SI4732 SSB patch from your own stock firmware file (Ferrite images only)")
    ap.add_argument("--marker", metavar="MARKER.BIN",
                    help="the 16 bytes at 0x080F0000 recorded at backup time (default: marker.bin beside --backup)")
    ap.add_argument("--icons", metavar="ICONS.BIN",
                    help="stock's menu icons (161,472 bytes from 0x08080000, BACKUP-12.3) for a stock image: default "
                         "icons.bin beside --backup; without them the menu tiles stay blank")
    ap.add_argument("--as-is", action="store_true",
                    help="send a stock file unchanged (no padding over the marker sector's first page)")
    ap.add_argument("--dev-unsigned", action="store_true",
                    help="take an unsigned .ffw (a local build): releases are signed with the release key")
    ap.add_argument("--parts-only", action="store_true",
                    help="a .ffw's SPI parts only, to a radio already running that build (no MCU flash)")
    ap.add_argument("--pin", help="the radio's PIN, if it has a PIN lock (for writing the SPI parts)")
    a = ap.parse_args()
    if not a.dry_run:
        import ferrite_link
        try:
            a.port = ferrite_link.resolve_port(a.port, cable_only=True)  # a name, FERRITE_PORT or the only cable
        except ferrite_link.PortError as e:
            sys.exit(str(e))

    parts = []
    if a.file.lower().endswith(".ffw"):
        import ffw
        try:
            m, entries, signed = ffw.check(open(a.file, "rb"), a.dev_unsigned)
            parts = ffw.spi_parts(m, entries)
        except ffw.Refused as e:
            sys.exit("refusing the update file:\n  " + "\n  ".join(e.problems))
        print(ffw.describe(m, signed))
        if a.parts_only:
            if not a.dry_run:
                write_spi(a, parts)
            return
        data, is_btf = entries["firmware.btf"], True
    elif a.parts_only:
        sys.exit("--parts-only takes a .ffw")
    else:
        data = open(a.file, "rb").read()
        is_btf = a.file.upper().endswith(".BTF")
    img = btf.unpack(data)[0] if is_btf else data
    errs = btf.check_image(img, btf.BOOT_MAX if is_btf else btf.APP_MAX)
    if not parts:
        errs += check_sha256(a.file, data)
    if FERRITE_MARK in img:
        if a.backup:
            berrs, desc = check_backup(a.backup)
            errs += berrs
            if desc:
                print(desc)
        elif not a.dry_run:
            errs.append("a Ferrite image needs --backup: first read the whole SPI flash with the hwtest image "
                        "(fw/tools/flashdump.py PORT flash.bin), keep it safe, and pass it here")
        else:
            print("note: flashing this Ferrite image will need --backup FLASH.BIN")
    if errs:
        sys.exit("refusing to flash:\n  " + "\n  ".join(errs))
    mpath = marker_path(a)
    marker = load_marker(mpath)
    fb_marker, fb_icons = read_backup(a.backup)[1:] if a.backup and os.path.exists(a.backup) else (None, None)
    if marker is None and fb_marker is not None:  # a .fbin carries its own (decision 407)
        marker, mpath = fb_marker, a.backup
    stock = FERRITE_MARK not in img and READER_MARK not in img and RESTORER_MARK not in img
    if stock and not a.as_is:                  # BACKUP-12: the way back covers 0x080F0000's first page
        ipath = a.icons or (os.path.join(os.path.dirname(os.path.abspath(a.backup)), "icons.bin") if a.backup else None)
        icons = open(ipath, "rb").read() if ipath and os.path.exists(ipath) else None
        if icons is None and fb_icons is not None:  # a .fbin carries them (decision 407)
            icons, ipath = fb_icons, a.backup
        if icons is not None and not btf.icons_plausible(icons):
            sys.exit(f"{ipath}: not stock's menu icons ({btf.ICONS_LEN} bytes of tiles expected)")
        note = icons_note(img, icons, ipath)
        img = btf.stock_restore_image(img, marker, icons)
        is_btf = False                         # repacked with the zero key below
        print(f"stock image padded through 0x{btf.MARKER_ADDR + btf.MARKER_PAGE - 1:08X}; marker sector: "
              f"{btf.marker_describe(marker)}{f' ({mpath})' if marker is not None else ''}; menu icons: "
              f"{note}")
    if a.ssb_from:
        import hashlib
        import ssbpatch
        patch = ssbpatch.extract(a.ssb_from)
        if not patch:
            sys.exit(f"no SSB patch found in {a.ssb_from}")
        known = hashlib.sha256(patch).hexdigest() == ssbpatch.KNOWN_SHA256
        img, at = ssbpatch.bundle(img, patch)
        if len(img) > btf.APP_MAX:
            sys.exit("image plus SSB patch does not fit in MCU flash")
        print(f"SSB patch from {a.ssb_from}: {len(patch)} bytes, {'known' if known else 'UNKNOWN (Ferrite will ask)'}, "
              f"bundled at 0x{btf.APP_BASE + at:08X}")
        is_btf = False                         # repack below
    if FERRITE_MARK in img and len(img) > btf.MARKER_OFF:
        # BACKUP-12: this image overwrites the marker sector; its bytes must be in the backup's record first.
        print(f"this image reaches 0x{btf.MARKER_ADDR:08X} (stock's marker sector): "
              f"{btf.marker_describe(marker)}{f', recorded in {mpath}' if marker is not None else ''}")
    if not is_btf:
        data = btf.pack(img)
    blocks = [data[i:i + 1024].ljust(1024, b"\x00") for i in range(0, len(data), 1024)]   # 00 pad, as Radtel
    print(f"{a.file}: {btf.describe(img)}; {len(blocks)} blocks")
    if a.dry_run:
        return

    try:
        import serial
    except ImportError:
        sys.exit("needs pyserial: pip install pyserial")
    with serial.Serial(a.port, 115200, timeout=0.01) as port:
        if not enter_bootloader(port, a.wait):
            sys.exit("no answer from the bootloader")
        print("bootloader answered")
        steps = [(CMD_HELLO, 0, b""), (CMD_MODEL, 0, data[btf.MODEL_OFF:btf.MODEL_OFF + 32]),
                 (CMD_COUNT, 0, struct.pack(">H", max(len(blocks) - 1, 1)))]
        for cmd, seq, payload in steps:
            r = command(port, cmd, seq, payload)
            if r != ACK:
                sys.exit(f"command {cmd:02X}: {RESULTS.get(r, 'no reply') if r is not None else 'no reply'}")
        t0 = time.monotonic()
        for seq, blk in enumerate(blocks):
            r = command(port, CMD_DATA, seq, blk, timeout=3.0)
            if r != ACK:
                sys.exit(f"\nblock {seq}: {RESULTS.get(r, 'no reply') if r is not None else 'no reply'}; "
                         "the radio stays in the bootloader, run this again")
            if a.progress:
                print(f"\r{(seq + 1) * 100 // len(blocks):3d}%  block {seq + 1}/{len(blocks)}", end="", flush=True)
        r = command(port, CMD_END)
        print(f"{chr(10) if a.progress else ''}done in {time.monotonic() - t0:.0f} s{'' if r == ACK else ' (no reply to end; check the radio)'}; "
              "the radio restarts into the new image")
    if parts:
        write_spi(a, parts)


def write_spi(a, parts):
    """UPD-21: the .ffw's SPI parts, once Ferrite answers on the cable: written, read back, the layout stamped."""
    import ffw
    import ferrite_link
    print("waiting for Ferrite to start, then writing its SPI parts over the cable...")
    end = time.monotonic() + 45
    while True:
        try:
            r = ferrite_link.Radio(a.port, a.pin, name=b"flash.py")
            break
        except ferrite_link.RadioError as e:
            sys.exit(f"the radio refused the link ({e}): with a PIN lock, pass --pin")
        except ferrite_link.ProtocolError as e:
            sys.exit(f"Ferrite answered with link protocol {e.got}, these tools speak {ferrite_link.PROTOCOL} (decision 385):"
                     " write the parts with that release's own tools/flash.py --parts-only")
        except (TimeoutError, OSError, SystemExit):
            if time.monotonic() > end:
                sys.exit("no answer from Ferrite on the cable: once it runs, write the parts with --parts-only")
            time.sleep(2)
    try:
        ffw.write_parts(lambda ty, p: r.link.ask(ty, p), r.hello, parts)
    except ffw.Refused as e:
        sys.exit("SPI parts not written:\n  " + "\n  ".join(e.problems) +
                 "\n(the firmware runs without them: short messages, the plain terminal, no logo)")
    finally:
        r.close()


if __name__ == "__main__":
    main()
