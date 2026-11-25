#!/usr/bin/env python3
"""Your radios by name (~/.config/ferrite/radios.toml; the format: radios.example.toml).

    tools/radios.py list [--scan]        # the radios, their links, and which answer now (--scan: Bluetooth too)
    tools/radios.py add NAME [PORT]      # connect (PORT, else as the tools pick one), read its callsign and version,
                                         # and write its entry
    tools/radios.py flash [NAME] [--yes] # build its image (tools/mkbtf.sh) and flash it with its backup

Without radios.toml, flash takes FERRITE_PORT, FERRITE_BUILD (tx) and FERRITE_BACKUP."""
import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
FERRITE = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import ferrite_link as fl  # noqa: E402

IMAGES = {"receive": "ferrite", "tx": "ferrite-tx", "tx-bench": "ferrite-txbench"}  # tools/mkbtf.sh's builds


def config_path():
    return os.environ.get("FERRITE_RADIOS") or fl.RADIOS_TOML


def scan(seconds=4.0):
    """Bluetooth addresses seen in a short scan (empty without bleak)."""
    try:
        import asyncio
        from bleak import BleakScanner
    except ImportError:
        print("(no Bluetooth scan: bleak isn't installed; just setup)")
        return set()
    devs = asyncio.run(BleakScanner.discover(timeout=seconds))
    return {d.address.lower() for d in devs}


def cmd_list(a):
    cfg = fl.load_radios(config_path())
    radios = cfg.get("radios", {})
    cables = fl.cables()
    print(f"cables: {', '.join(cables) or 'none'}")
    if not radios:
        print(f"no radios in {config_path()} (radios.example.toml shows the format; just radio-add NAME)")
        return 0
    seen = scan() if a.scan else None
    for name, r in sorted(radios.items()):
        links = []
        if r.get("cable"):
            links.append(f"cable {r['cable']}" if isinstance(r["cable"], str) else "cable")
        if r.get("ble"):
            tag = "" if seen is None else (" (seen)" if r["ble"].lower() in seen else " (not seen)")
            links.append(f"ble {r['ble']}{tag}")
        try:
            now = fl.resolve_port(name, env={}, radios=cfg)
        except fl.PortError as e:
            now = f"none: {e}"
        mark = "*" if cfg.get("default") == name else " "
        extra = ", ".join(x for x in (r.get("callsign"), r.get("version"), r.get("note")) if x)
        print(f"{mark} {name:10s} {r.get('build', 'tx'):9s} {'; '.join(links) or 'no links':40s} now: {now}"
              + (f"\n  {'':10s} {extra}" if extra else ""))
    return 0


def toml_value(v):
    return "true" if v is True else "false" if v is False else json.dumps(v)


def cmd_add(a):
    path = config_path()
    cfg = fl.load_radios(path)
    if a.name in cfg.get("radios", {}):
        sys.exit(f"{a.name} is already in {path}: edit it there")
    if not a.name.replace("-", "").replace("_", "").isalnum():
        sys.exit(f"{a.name}: letters, digits, - and _ only")
    try:
        port = fl.resolve_port(a.port)
    except fl.PortError as e:
        sys.exit(str(e))
    print(f"connecting on {port} ...")
    with fl.Radio(port, a.pin, name=b"radios.py") as r:
        info = fl.read_info(r.link)
    build = "tx-bench" if info["bench"] else "tx" if info["build"] in ("tx", "tx+dry", "tx-dry") else "receive"
    entry = {"build": build}
    if port.startswith("ble:"):
        entry["ble"] = port[4:]
    elif not port.startswith("tcp:"):
        entry["cable"] = True
    entry.update(backup=f"~/ferrite-backups/{a.name}.bin", confirm_flash=True,
                 callsign=info["callsign"], version=info["version"], note="")
    lines = [f"\n[radios.{a.name}]"] + [f"{k} = {toml_value(v)}" for k, v in entry.items()]
    os.makedirs(os.path.dirname(path), exist_ok=True)
    new = not os.path.exists(path)
    with open(path, "a") as f:
        if new:
            f.write(f'# Ferrite radios (radios.example.toml in the ferrite repo shows every key)\ndefault = "{a.name}"\n')
        f.write("\n".join(lines) + "\n")
    print(f"{a.name}: {info['callsign'] or 'no callsign'}, firmware {info['version']} ({info['build']}) -> {path}")
    print(f"  check its backup path ({entry['backup']}): flashing refuses without that radio's whole-chip backup")
    return 0


def cmd_flash(a):
    cfg = fl.load_radios(config_path())
    name = a.name
    if name in fl.AUTO:  # as resolve_port picks: FERRITE_PORT, then the default
        env = os.environ.get("FERRITE_PORT")
        name = env if env not in fl.AUTO else cfg.get("default") or "auto"
    r = {}
    if name not in fl.AUTO and not fl.is_port(name):
        r = cfg.get("radios", {}).get(name.partition(":")[0])
        if r is None:
            sys.exit(f"{name}: not in {config_path()}")
    build = r.get("build") or os.environ.get("FERRITE_BUILD") or "tx"
    if build not in IMAGES:
        sys.exit(f"build {build!r}: one of {', '.join(IMAGES)}")
    backup = os.path.expanduser(r.get("backup") or os.environ.get("FERRITE_BACKUP") or "")
    if not backup or not os.path.exists(backup):
        sys.exit(f"no backup at {backup or '(none set)'}: give the radio a `backup` in radios.toml or FERRITE_BACKUP "
                 "(its whole SPI flash; flash.py refuses without it)")
    try:
        port = fl.resolve_port(name, cable_only=True)
    except fl.PortError as e:
        sys.exit(str(e))
    image = os.path.join(FERRITE, "target", IMAGES[build] + ".ffw")
    if r.get("confirm_flash") and not a.yes:
        if input(f"flash {build} to {name} on {port}? [y/N] ").strip().lower() != "y":
            sys.exit("not flashed")
    mk = subprocess.run([os.path.join(HERE, "mkbtf.sh")] + ([] if build == "receive" else [build]), cwd=FERRITE,
                        capture_output=True, text=True)
    sys.stdout.write("".join((mk.stdout + mk.stderr).splitlines(True)[-4:]))
    if mk.returncode:
        sys.exit(f"tools/mkbtf.sh {build} failed")
    return subprocess.run([sys.executable, os.path.join(HERE, "flash.py"), port, image, "--backup", backup,
                           "--dev-unsigned"]).returncode


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list")
    p.add_argument("--scan", action="store_true", help="a 4 s Bluetooth scan for the radios' IDs")
    p = sub.add_parser("add")
    p.add_argument("name")
    p.add_argument("port", nargs="?", help="default: FERRITE_PORT, radios.toml's default, the only cable")
    p.add_argument("--pin", help="the radio's PIN, over Bluetooth with a PIN lock")
    p = sub.add_parser("flash")
    p.add_argument("name", nargs="?", default="auto")
    p.add_argument("--yes", action="store_true", help="don't ask, even with confirm_flash")
    a = ap.parse_args()
    return {"list": cmd_list, "add": cmd_add, "flash": cmd_flash}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
