#!/usr/bin/env python3
"""Build report for a release (Alex, Oct 23, 2026): one HTML page (and the same facts as JSON) beside each image.

    python3 tools/buildreport.py tx                      # build ferrite-tx, run the host tests, write the report
    python3 tools/buildreport.py tx --ui RESULTS.json    # with the sim UI tests' results (tools/uitest.py --sim)
    python3 tools/buildreport.py tx --no-build --no-tests
    python3 tools/buildreport.py tx --from-logs HOST.log DRIVERS.log   # tests already run under cargo llvm-cov (CI)
    python3 tools/buildreport.py tx --sig FILE.ffw                     # the update file's signature (CI)

--from-logs takes the output of `cargo llvm-cov nextest --no-report` (or `cargo llvm-cov --no-report`) (the workspace, then the drivers with --all-features)
saved by the caller: the counts come from those logs and the coverage from that run's data, so nothing runs twice.

What it shows:
  - code coverage: lines the host tests run (cargo llvm-cov), per crate; the headline counts only code that ships on
    the radio (the PC-only simulators are left out); the firmware crate runs only on the radio and isn't measured;
  - the build: version, commit (and whether the tree had changes), toolchain, image size, CRC-32 and SHA-256;
  - tests: host tests (workspace, and the drivers with their features), the sim UI tests from --ui (with the
    commit they ran on), and tools/slowcheck.py;
  - static RAM: .data and .bss against the 96 KB the MCU has, and what is left for the stack;
  - MCU flash: the fast part (zero-wait, up to 256 KB from the chip's start) and the slow part, against the 1,012 KB
    the bootloader leaves us (decision 338);
  - SPI flash: the map, read from crates/ferrite-core/src/layout.rs (each region, its size, who owns it, whether
    Ferrite may write it).
Output: target/build-report-<build>.html and .json. Needs arm-none-eabi-nm (as slowcheck.py) and, for coverage,
cargo-llvm-cov (`cargo install cargo-llvm-cov`; --no-coverage skips it, about 4 minutes).
"""
import argparse, base64, datetime, hashlib, html, json, os, re, shutil, subprocess, sys, zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ELF = os.path.join(ROOT, "target/thumbv7em-none-eabihf/release/ferrite")
LAYOUT = os.path.join(ROOT, "crates/ferrite-core/src/layout.rs")
BUILDS = {"rx": ("", "ferrite"), "tx": ("tx", "ferrite-tx"), "tx-bench": ("tx-bench", "ferrite-txbench")}
APP_BASE, APP_SIZE = 0x0800_3000, 1012 * 1024  # firmware/memory.x (decision 338)
FAST_END = 0x0804_0000  # zero-wait flash ends 256 KB into the chip (memory.x)
RAM_BASE, RAM_SIZE = 0x2000_0000, 96 * 1024
SPI_SIZE = 4 << 20
DRIVERS = ["uradio-bus", "bk4829", "si4732", "st7789", "w25q", "keypad", "nmea"]


def run(*a, check=True):
    return subprocess.run(a, cwd=ROOT, capture_output=True, text=True, check=check)


def kb(n):
    return f"{n / 1024:.1f} KB"


# ---- the build ------------------------------------------------------------------------------------------------------

def build_info(build):
    flavour, stem = BUILDS[build]
    btf, bin_ = (os.path.join(ROOT, "target", stem + ext) for ext in (".btf", ".bin"))
    img = open(bin_, "rb").read()
    commit = os.environ.get("FERRITE_COMMIT") or run("git", "rev-parse", "--short", "HEAD").stdout.strip()
    dirty = bool(run("git", "status", "--porcelain", "--", ".").stdout.strip())
    version = re.search(r'^version = "([^"]+)"', open(os.path.join(ROOT, "Cargo.toml")).read(), re.M).group(1)
    # SELF-5.1: About's "Unofficial build" label is in the image unless the release workflow built it (FERRITE_OFFICIAL=1)
    official = b"Unofficial build" not in img
    return dict(build=build, version=version, commit=commit, dirty=dirty, official=official,
                date=datetime.datetime.now().strftime("%Y-%m-%d %H:%M"),
                rustc=run("rustc", "-V").stdout.strip(), image=os.path.basename(btf),
                image_bytes=os.path.getsize(btf), crc32=f"{zlib.crc32(img):08X}",
                sha256=hashlib.sha256(open(btf, "rb").read()).hexdigest())


def signature(path, btf):
    """The image's minisign signature (--sig): its key ID, trusted comment and text, and whether it verifies against
    minisign.pub (needs the minisign tool; None when it isn't installed). A signed `.ffw` (decision 335): the
    signature over its manifest, checked by tools/ffw.py."""
    if path.endswith(".ffw"):
        sys.path.insert(0, os.path.join(ROOT, "tools"))
        import ffw
        mbytes, sig, _ = ffw.read(open(path, "rb"))
        if sig is None:
            return None
        text = sig.decode()
        lines = text.splitlines()
        raw = base64.b64decode(lines[1])
        ok = not ffw.minisign_verify(open(os.path.join(ROOT, "minisign.pub")).read(), mbytes, sig)
        trusted = next((l.split(":", 1)[1].strip() for l in lines if l.startswith("trusted comment:")), "")
        return dict(file=os.path.basename(path) + " (manifest.minisig)", key_id=raw[2:10][::-1].hex().upper(),
                    trusted_comment=trusted, verified=ok, text=text)
    text = open(path).read()
    lines = text.splitlines()
    raw = base64.b64decode(lines[1])
    trusted = next((l.split(":", 1)[1].strip() for l in lines if l.startswith("trusted comment:")), "")
    ok = None
    if shutil.which("minisign"):
        ok = run("minisign", "-Vqm", btf, "-x", path, "-p", os.path.join(ROOT, "minisign.pub"), check=False).returncode == 0
    return dict(file=os.path.basename(path), key_id=raw[2:10][::-1].hex().upper(), trusted_comment=trusted,
                verified=ok, text=text)


# ---- tests ----------------------------------------------------------------------------------------------------------

def cargo_tests(*args):
    out = run("cargo", "test", *args, check=False)
    return test_counts(out.stdout + out.stderr, out.returncode == 0)


def test_counts(text, ok):
    tot = dict(passed=0, failed=0, ignored=0)
    for m in re.finditer(r"test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored", text):
        for k, v in zip(tot, m.groups()):
            tot[k] += int(v)
    # cargo nextest's summary: "Summary [ 28.9s] 2016 tests run: 2016 passed, 2 failed, 38 skipped"
    for m in re.finditer(r"Summary \[[^\]]*\] \d+ tests? run: ([^\n]*)", text):
        for n, k in re.findall(r"(\d+) (passed|failed|skipped)", m.group(1)):
            tot["ignored" if k == "skipped" else k] += int(n)
    tot["ok"] = ok and tot["failed"] == 0 and tot["passed"] > 0
    return tot


def logged_tests(path):
    """The counts in a saved test log (--from-logs): ok when every suite in it said ok."""
    text = open(path, errors="replace").read()
    return test_counts(text, "test result: FAILED" not in text and "error: test run failed" not in text)


def ui_tests(path):
    d = json.load(open(path))
    rules = d.get("rules", {})
    by = lambda r: sorted(k for k, v in rules.items() if v["result"] == r)
    return dict(counts=d["counts"], commit=d.get("commit", "").split()[0], build=d.get("build"),
                started=d.get("started"), rules_pass=len(by("pass")), rules_fail=by("fail"),
                rules_xfail=by("xfail"))


PC_ONLY = ("sim", "sim-web")  # host-only crates: left out of the coverage section (kept in the JSON)


def coverage(ran=False):
    """Line coverage of the host tests (the workspace, then the drivers with their features), per crate; `ran`: the
    caller already ran them under cargo llvm-cov --no-report (--from-logs), so only the report is made."""
    if not ran:
        run("cargo", "llvm-cov", "clean", "--workspace", check=False)
        for args in ([], sum((["-p", d] for d in DRIVERS), []) + ["--all-features"]):
            if run("cargo", "llvm-cov", "--no-report", *args, check=False).returncode:
                return None
    out = run("cargo", "llvm-cov", "report", "--json", "--summary-only", check=False)
    if out.returncode:
        return None
    crates = {}
    for f in json.loads(out.stdout)["data"][0]["files"]:
        p = os.path.relpath(f["filename"], ROOT).split(os.sep)
        if p[0].startswith(".."):
            continue
        name = p[1] if p[0] in ("crates", "drivers", "digimodes") else p[0]
        c = crates.setdefault(name, dict(lines=0, covered=0))
        c["lines"] += f["summary"]["lines"]["count"]
        c["covered"] += f["summary"]["lines"]["covered"]
    tot = lambda keep: (sum(c["lines"] for n, c in crates.items() if keep(n)), sum(c["covered"] for n, c in crates.items() if keep(n)))
    radio, every = tot(lambda n: n not in PC_ONLY), tot(lambda n: True)
    return dict(crates=crates, radio_lines=radio[0], radio_covered=radio[1], all_lines=every[0], all_covered=every[1])


# ---- MCU flash and RAM ----------------------------------------------------------------------------------------------

def elf_sizes():
    syms = {}
    for line in run("arm-none-eabi-nm", ELF).stdout.splitlines():
        p = line.split()
        if len(p) == 3:
            syms[p[2]] = int(p[0], 16)
    secs = {}
    for line in run("arm-none-eabi-size", "-A", ELF).stdout.splitlines():
        p = line.split()
        if len(p) == 3 and p[0].startswith(".") and p[1].isdigit() and int(p[1]) and not p[0].startswith((".debug", ".comment", ".ARM")):
            secs[p[0]] = int(p[1])
    flash_secs = {k: v for k, v in secs.items() if k not in (".bss", ".uninit")}
    image = sum(flash_secs.values())
    fast = syms["__sslow_rodata"] - APP_BASE
    data, bss = secs.get(".data", 0), secs.get(".bss", 0) + secs.get(".uninit", 0)
    return dict(sections=secs, image=image, fast=fast, fast_budget=FAST_END - APP_BASE, slow=image - fast,
                slow_budget=APP_SIZE - (FAST_END - APP_BASE), app_size=APP_SIZE,
                ram_data=data, ram_bss=bss, ram_static=data + bss, ram_size=RAM_SIZE, ram_stack=RAM_SIZE - data - bss)


# ---- SPI flash ------------------------------------------------------------------------------------------------------

OWNER = {"CALIBRATION": "factory", "UPDATE_AREA": "bootloader", "SSB_PATCH": "owner's stock firmware", "FREE": "free",
         "CONFIG_FREE": "free"}
# Ranges in layout.rs that aren't regions of the map (decision 333: layout 2's header place, the raw data range).
NOT_REGIONS = {"V2_HEADER", "DATA"}


def spi_map():
    src = open(LAYOUT).read()
    regions, doc = [], []
    for line in src.splitlines():
        s = line.strip()
        if s.startswith("///"):
            doc.append(s[3:].strip())
            continue
        m = re.match(r"pub const (\w+): Range<u32> = 0x([\w_]+)\.\.0x([\w_]+);", s)
        if m and m.group(1) not in NOT_REGIONS:
            a, b = (int(x.replace("_", ""), 16) for x in m.group(2, 3))
            regions.append(dict(name=m.group(1), start=a, end=b, size=b - a, note=" ".join(doc)))
        doc = [] if not s.startswith("///") else doc
    at = {r["name"]: r for r in regions}

    def addr(e):  # REGION.start or REGION.end
        n, edge = e.strip().split(".")
        return at[n][edge]
    w = re.search(r"pub static WRITABLE: \[Range<u32>; \d+\] =\s*\[(.*?)\];", src, re.S).group(1)
    writable = [tuple(addr(x) for x in part.split("..")) for part in w.split(",") if ".." in part]
    for r in regions:
        r["writable"] = any(a <= r["start"] and r["end"] <= b for a, b in writable)
        r["owner"] = OWNER.get(r["name"], "Ferrite")
    # the gaps: never written by Ferrite (stock's or empty); layout 3 leaves none
    out, at_ = [], 0
    for r in sorted(regions, key=lambda r: r["start"]):
        if r["start"] > at_:
            out.append(dict(name="(unmapped)", start=at_, end=r["start"], size=r["start"] - at_, note="not used by Ferrite",
                            writable=False, owner="untouched"))
        out.append(r)
        at_ = r["end"]
    if at_ < SPI_SIZE:
        out.append(dict(name="(unmapped)", start=at_, end=SPI_SIZE, size=SPI_SIZE - at_, note="", writable=False,
                        owner="untouched"))
    ferrite = sum(r["size"] for r in out if r["writable"] and r["owner"] == "Ferrite")
    return dict(regions=out, size=SPI_SIZE, ferrite_bytes=ferrite,
                writable_bytes=sum(r["size"] for r in out if r["writable"]))


# ---- the page -------------------------------------------------------------------------------------------------------

CSS = """
:root{--bg:#fbfaf7;--fg:#1d1b18;--muted:#6b665d;--line:#e3ded4;--card:#fff;--fast:#c2410c;--slow:#f0a46b;--free:#ece7dd;
--ok:#15803d;--bad:#b91c1c;--ferrite:#c2410c;--stock:#64748b;--factory:#7c3aed;--boot:#0f766e;--freec:#d6d0c4}
@media (prefers-color-scheme:dark){:root{--bg:#16140f;--fg:#ece8df;--muted:#a39d91;--line:#2f2b24;--card:#1e1b16;
--free:#2a261f;--freec:#3a352c;color-scheme:dark}}
body{background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif;margin:0;padding:24px 16px}
main{max-width:860px;margin:auto}h1{font-size:26px;margin:0 0 4px}h2{font-size:18px;margin:28px 0 8px}
.sub{color:var(--muted)}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
td,th{padding:5px 8px;border-bottom:1px solid var(--line);text-align:left;vertical-align:top}th{color:var(--muted);font-weight:600}
td.n{text-align:right;white-space:nowrap}.wrap{overflow-x:auto}.bar{display:flex;height:22px;border-radius:4px;overflow:hidden;margin:8px 0;background:var(--free)}
.bar i{display:block;height:100%}.ok{color:var(--ok);font-weight:600}.bad{color:var(--bad);font-weight:600}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:10px}.card{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:10px 12px}
.card b{display:block;font-size:22px}.key span{display:inline-block;width:10px;height:10px;border-radius:2px;margin:0 4px 0 12px}
code{font-size:13px}pre{font-size:12px;overflow-x:auto;white-space:pre}@media print{body{background:#fff;color:#000}}
"""


def bar(parts):
    return '<div class="bar">' + "".join(f'<i style="width:{100 * f:.2f}%;background:var({c})" title="{html.escape(t)}"></i>'
                                         for f, c, t in parts) + "</div>"


def page(r):
    b, t, m, s = r["build"], r["tests"], r["mcu"], r["spi"]
    e = html.escape
    res = lambda ok: '<span class="ok">pass</span>' if ok else '<span class="bad">FAIL</span>'
    out = [f"<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'>"
           f"<title>Ferrite {e(b['version'])} {e(b['build'])} build report</title><style>{CSS}</style><main>",
           f"<h1>Ferrite {e(b['version'])}, {e(b['build'])} build</h1>",
           f"<p class=sub>{'Official release build' if b.get('official') else 'Unofficial build'} · "
           f"commit <code>{e(b['commit'])}</code>{' (with uncommitted changes)' if b['dirty'] else ''} · "
           f"{e(b['date'])} · {e(b['rustc'])}</p>",
           "<div class=grid>",
           f"<div class=card>Image<b>{kb(b['image_bytes'])}</b>{e(b['image'])}, image CRC-32 {b['crc32']} (as mkbtf.sh's first line)</div>",
           f"<div class=card>Fast flash<b>{100 * m['fast'] / m['fast_budget']:.0f} %</b>{kb(m['fast'])} of {kb(m['fast_budget'])}</div>",
           f"<div class=card>Slow flash<b>{100 * m['slow'] / m['slow_budget']:.0f} %</b>{kb(m['slow'])} of {kb(m['slow_budget'])}</div>",
           f"<div class=card>Total flash<b>{100 * m['image'] / m['app_size']:.0f} %</b>{kb(m['image'])} of {kb(m['app_size'])}</div>",
           f"<div class=card>Static RAM<b>{kb(m['ram_static'])}</b>{kb(m['ram_stack'])} left for the stack</div>"]
    if t:
        host = t["host"]
        tot = host["passed"] + t["drivers"]["passed"] + (t["ui"]["counts"]["pass"] if t.get("ui") else 0)
        bad = host["failed"] + t["drivers"]["failed"] + (t["ui"]["counts"]["fail"] + t["ui"]["counts"]["error"] if t.get("ui") else 0)
        out.append(f"<div class=card>Tests<b class={'ok' if not bad else 'bad'}>{tot} pass</b>{bad} failed</div>")
    cov = r.get("coverage")
    if cov:
        out.append(f"<div class=card>Code coverage<b>{100 * cov['radio_covered'] / cov['radio_lines']:.1f} %</b>"
                   f"of the radio's host-tested lines</div>")
    out.append("</div>")
    out.append(f"<p class=sub>SHA-256 <code>{b['sha256']}</code></p>")
    sig = r.get("signature")
    if sig:
        state = {True: "<b class=ok>verifies</b> against minisign.pub", False: "<b class=bad>does not verify</b> against "
                 "minisign.pub", None: "not checked (no minisign tool)"}[sig["verified"]]
        out.append(f"<p class=sub>Signed with minisign, key ID <code>{sig['key_id']}</code>, trusted comment "
                   f"“{e(sig['trusted_comment'])}”: {state}. {e(sig['file'])}:</p><pre>{e(sig['text'])}</pre>")
    else:
        out.append("<p class=sub>Not signed (a local build).</p>")

    if t:
        out.append("<h2>Tests</h2><div class=wrap><table><tr><th>Suite</th><th class=n>Passed</th><th class=n>Failed</th><th class=n>Other</th><th>Result</th></tr>")
        for name, x in (("Host tests (workspace)", t["host"]), ("Drivers, all features", t["drivers"])):
            out.append(f"<tr><td>{name}</td><td class=n>{x['passed']}</td><td class=n>{x['failed']}</td>"
                       f"<td class=n>{x['ignored']} ignored</td><td>{res(x['ok'] and not x['failed'])}</td></tr>")
        if t.get("ui"):
            u, c = t["ui"], t["ui"]["counts"]
            same = u["commit"].startswith(b["commit"]) or b["commit"].startswith(u["commit"])
            out.append(f"<tr><td>Sim UI tests ({e(u['build'] or '')}, commit <code>{e(u['commit'])}</code>"
                       f"{'' if same else ', <span class=bad>not this commit</span>'})</td><td class=n>{c['pass']}</td>"
                       f"<td class=n>{c['fail'] + c['error']}</td><td class=n>{c['skip']} skipped, {c['xfail']} known</td>"
                       f"<td>{res(not c['fail'] and not c['error'])}</td></tr>")
        out.append(f"<tr><td>Fast-flash check (slowcheck)</td><td class=n></td><td class=n></td><td class=n></td><td>{res(t['slowcheck'])}</td></tr>")
        out.append("</table></div>")
        if t.get("ui"):
            u = t["ui"]
            out.append(f"<p class=sub>Spec rules covered by the UI tests: {u['rules_pass']} pass"
                       + (f"; failing: {e(', '.join(u['rules_fail']))}" if u["rules_fail"] else "")
                       + (f"; known deviations: {e(', '.join(u['rules_xfail']))}" if u["rules_xfail"] else "") + ".</p>")

    if cov:
        pct = lambda c, n: f"{100 * c / n:.1f} %" if n else "-"
        out.append("<h2>Code coverage</h2>")
        out.append(f"<p class=sub>Lines the host tests run (cargo llvm-cov): {cov['radio_covered']:,} of {cov['radio_lines']:,} "
                   f"({pct(cov['radio_covered'], cov['radio_lines'])}) in code that ships on the radio (the PC-only simulators "
                   "aren't counted or listed). The firmware crate (Radio task, HAL) "
                   "runs only on the radio and isn't measured: the sim UI and on-radio tests cover it.</p>")
        out.append("<div class=wrap><table><tr><th>Crate</th><th class=n>Lines</th><th class=n>Not run</th><th class=n>Covered</th><th></th></tr>")
        for n, c in sorted(((n, c) for n, c in cov["crates"].items() if n not in PC_ONLY), key=lambda x: -x[1]["lines"]):
            out.append(f"<tr><td>{e(n)}</td><td class=n>{c['lines']:,}</td>"
                       f"<td class=n>{c['lines'] - c['covered']:,}</td><td class=n>{pct(c['covered'], c['lines'])}</td>"
                       f"<td style='min-width:6rem'>{bar([(c['covered'] / c['lines'] if c['lines'] else 0, '--fast', 'covered')])}</td></tr>")
        out.append("</table></div>")

    hist = r.get("history") or []
    if hist:
        out.append("<h2>Image size by merge</h2><div class=wrap><table><tr><th>Date</th><th>Commit</th><th class=n>Image</th>"
                   "<th class=n>Change</th><th class=n>Fast</th><th class=n>Slow</th></tr>")
        prev = None
        for h in hist:
            ch = "" if prev is None else f"{(int(h['bytes']) - prev) / 1024:+.1f} KB"
            out.append(f"<tr><td>{e(h['date'])}</td><td><code>{e(h['commit'])}</code></td><td class=n>{kb(int(h['bytes']))}</td>"
                       f"<td class=n>{ch}</td><td class=n>{kb(int(h['fast']))}</td><td class=n>{kb(int(h['slow']))}</td></tr>")
            prev = int(h["bytes"])
        out.append("</table></div><p class=sub>From tools/size-history.tsv: each build on a clean main records itself "
                   "(tools/sizelog.py, run by mkbtf.sh), which warns past 4 KB of growth.</p>")

    out.append("<h2>Static RAM</h2>")
    out.append(bar([(m["ram_data"] / m["ram_size"], "--fast", ".data"), (m["ram_bss"] / m["ram_size"], "--slow", ".bss")]))
    out.append(f"<div class=key><span style='background:var(--fast);margin-left:0'></span>.data {kb(m['ram_data'])}"
               f"<span style='background:var(--slow)'></span>.bss {kb(m['ram_bss'])}"
               f"<span style='background:var(--free)'></span>stack and free {kb(m['ram_stack'])} of {kb(m['ram_size'])}</div>")

    out.append("<h2>MCU flash</h2>")
    app = m["app_size"]
    out.append(bar([(m["fast"] / app, "--fast", "fast used"), ((m["fast_budget"] - m["fast"]) / app, "--freec", "fast free"),
                    (m["slow"] / app, "--slow", "slow used")]))
    out.append(f"<div class=key><span style='background:var(--fast);margin-left:0'></span>fast {kb(m['fast'])} of {kb(m['fast_budget'])}"
               f"<span style='background:var(--freec)'></span>fast free {kb(m['fast_budget'] - m['fast'])}"
               f"<span style='background:var(--slow)'></span>slow {kb(m['slow'])} of {kb(m['slow_budget'])}"
               f"<span style='background:var(--free)'></span>free {kb(app - m['image'])}</div>")
    out.append("<div class=wrap><table><tr><th>Section</th><th class=n>Bytes</th><th>Where</th></tr>")
    where = {".text": "fast", ".rodata": "fast", ".fast_ui": "fast (drawing)", ".vector_table": "fast", ".slow_text": "slow",
             ".slow_rodata": "slow", ".data": "flash and RAM", ".bss": "RAM", ".uninit": "RAM"}
    for k, v in m["sections"].items():
        out.append(f"<tr><td><code>{e(k)}</code></td><td class=n>{v:,}</td><td>{where.get(k, 'fast')}</td></tr>")
    out.append("</table></div>")

    out.append("<h2>SPI flash (4 MB)</h2>")
    col = {"Ferrite": "--ferrite", "stock": "--stock", "factory": "--factory", "bootloader": "--boot", "free": "--freec",
           "owner's stock firmware": "--stock", "untouched": "--free"}
    out.append(bar([(x["size"] / s["size"], col[x["owner"]], f"{x['name']} {kb(x['size'])}") for x in s["regions"]]))
    out.append("<div class=key>" + "".join(f"<span style='background:var({c})'></span>{n}" for n, c in
               (("Ferrite", "--ferrite"), ("stock", "--stock"), ("factory", "--factory"), ("bootloader", "--boot"),
                ("held free", "--freec"), ("not used", "--free"))) + "</div>")
    out.append(f"<p class=sub>Ferrite may write {kb(s['writable_bytes'])} ({kb(s['ferrite_bytes'])} of it in its own records);"
               " the flash driver refuses everything else.</p>")
    out.append("<div class=wrap><table><tr><th>Region</th><th>Start</th><th>End</th><th class=n>Size</th><th>Owner</th><th>Writes</th><th>Note</th></tr>")
    for x in s["regions"]:
        out.append(f"<tr><td>{e(x['name'])}</td><td><code>0x{x['start']:06X}</code></td><td><code>0x{x['end']:06X}</code></td>"
                   f"<td class=n>{kb(x['size'])}</td><td>{e(x['owner'])}</td><td>{'yes' if x['writable'] else 'never'}</td>"
                   f"<td>{e(x['note'])}</td></tr>")
    out.append("</table></div></main>")
    return "\n".join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("build", choices=list(BUILDS))
    ap.add_argument("--ui", metavar="RESULTS.json", help="the sim UI tests' results.json (tools/uitest.py --sim)")
    ap.add_argument("--no-build", action="store_true", help="use the image as last built")
    ap.add_argument("--no-tests", action="store_true", help="skip the host tests")
    ap.add_argument("--no-coverage", action="store_true", help="skip code coverage (cargo llvm-cov, about 4 minutes)")
    ap.add_argument("--sig", metavar="IMAGE.btf.minisig", help="the image's minisign signature, or the signed .ffw, "
                    "shown in the report")
    ap.add_argument("--from-logs", nargs=2, metavar=("HOST.log", "DRIVERS.log"),
                    help="the host and driver tests already run under cargo llvm-cov --no-report: count them from "
                    "these logs and report that run's coverage instead of running anything again")
    a = ap.parse_args()
    if not a.no_build:
        run("tools/mkbtf.sh", *([BUILDS[a.build][0]] if BUILDS[a.build][0] else []))
    slow = run(sys.executable, "tools/slowcheck.py", check=False)
    r = dict(build=build_info(a.build), mcu=elf_sizes(), spi=spi_map(), tests=None)
    if a.sig:
        sig = signature(a.sig, os.path.join(ROOT, "target", BUILDS[a.build][1] + ".btf"))
        if sig:
            r["signature"] = sig
    if not a.no_tests:
        if a.from_logs:
            host, drivers = (logged_tests(f) for f in a.from_logs)
        else:
            host, drivers = cargo_tests(), cargo_tests(*sum((["-p", d] for d in DRIVERS), []), "--all-features")
        r["tests"] = dict(host=host, drivers=drivers, ui=ui_tests(a.ui) if a.ui else None, slowcheck=slow.returncode == 0)
    if not a.no_coverage:
        r["coverage"] = coverage(ran=bool(a.from_logs))
        if r["coverage"] is None:
            print("coverage: cargo llvm-cov failed or isn't installed (cargo install cargo-llvm-cov)", file=sys.stderr)
    hist = os.path.join(ROOT, "tools/size-history.tsv")
    if os.path.exists(hist):
        keys = open(hist).readline().strip().split("\t")
        r["history"] = [dict(zip(keys, l.split("\t"))) for l in open(hist).read().splitlines()[1:]
                        if l.split("\t")[2:3] == [BUILDS[a.build][1]]][-15:]
    stem = os.path.join(ROOT, "target", f"build-report-{a.build}")
    open(stem + ".json", "w").write(json.dumps(r, indent=1))
    open(stem + ".html", "w").write(page(r))
    m = r["mcu"]
    print(f"{stem}.html: {r['build']['image']} {r['build']['commit']}, fast {kb(m['fast'])}/{kb(m['fast_budget'])}, "
          f"slow {kb(m['slow'])}, RAM {kb(m['ram_static'])}/{kb(m['ram_size'])}"
          + (f", coverage {100 * r['coverage']['radio_covered'] / r['coverage']['radio_lines']:.1f} %" if r.get("coverage") else ""))


if __name__ == "__main__":
    main()
