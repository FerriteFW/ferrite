"""Load a channel file (`zone`/`ch` terminal lines, as `dump` writes them) into a radio over the link, one line
at a time. Each line goes through the terminal's own `ch`/`zone` commands (TERM rules): validated as an edit on the
radio would be, then saved by the store; nothing is written to flash directly.

    .venv/bin/python tools/loadch.py PORT FILE [--dry]        # PORT: the cable, ble:ID, a radio's name or auto

Stops at the first line the radio refuses. Lists channels and zones already on the radio that the file doesn't
set (they are left as they are; delete them on the radio if unwanted). --dry checks the file and lists only."""
import argparse, os, re, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ferrite_link as fl  # noqa: E402


def lines(path):
    for l in open(path):
        l = l.strip()
        if l and not l.startswith("#"):
            yield l


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("port")
    ap.add_argument("file")
    ap.add_argument("--dry", action="store_true")
    a = ap.parse_args()
    want = list(lines(a.file))
    nums = {(m.group(1), int(m.group(2))) for l in want if (m := re.match(r"(ch|zone) (\d+) ", l))}
    r = fl.Radio(a.port)
    print(f"radio: build {r.build}")
    have = [l for l in r.cmd("dump").splitlines() if re.match(r"(ch|zone) \d+ ", l)]
    extra = [l for l in have if (lambda m: (m.group(1), int(m.group(2))))(re.match(r"(ch|zone) (\d+) ", l)) not in nums]
    if extra:
        print(f"on the radio and not in the file ({len(extra)}), left as they are:")
        for l in extra:
            print("   ", l)
    if a.dry:
        print(f"dry: {len(want)} lines would be sent")
        return
    for i, l in enumerate(want, 1):
        out = r.cmd(l).strip()
        if "invalid" in out.lower() or "can't" in out.lower() or "no such" in out.lower():
            sys.exit(f"line {i} refused: {l}\n  -> {out}")
        print(f"\r{i}/{len(want)}", end="", flush=True)
    print(f"\nloaded {len(want)} lines")


if __name__ == "__main__":
    main()
