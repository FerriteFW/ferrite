#!/usr/bin/env python3
"""The test plan: every numbered rule in docs/manual/ (the chapter files), the automated tests that cover it, and the manual
(radio or bench) steps for the rest.

    python3 tools/testplan.py              # writes ../docs/test-plan.md
    python3 tools/testplan.py --init       # adds a draft entry to docs/test-plan/*.md for every rule without one
    python3 tools/testplan.py --coverage   # short report; exit 1 if a rule has neither a test nor an entry, or a
                                           # test cites a rule the manual doesn't have

A rule is covered by an automated test when its ID (e.g. SCAN-21) appears in the test's /// doc comment, its
attributes, its name or a comment inside its body. Ranges such as "SCAN-16 – SCAN-19" count every rule in them.
Tests are #[test] functions in ferrite (crates, drivers, sim, sim-web, firmware, digimodes) and the udigimodes crates; one
under sim/ or sim-web/ counts as a simulator test, the rest as host tests.

The hand-kept steps live in docs/test-plan/<chapter>.md, one entry per rule:

    ### TX-12
    Method: bench
    Equipment: dummy load, RF power meter
    Steps: ...
    Pass: ...
    Results:
    | Date | Firmware | Result | Notes |
    | --- | --- | --- | --- |
    | 2026-10-12 | 1a2b3c4 | pass | |

Method is one or more of host, sim, radio, bench, n/a (a moved or retired rule). --init never rewrites an entry
that exists; edit them by hand.

A rule that mixed several claims is split into sub-rules (decision 160): TX-30.1, TX-30.2, ... on lines nested under
the parent (`>     - **TX-30.1** ...`), each checkable by one method, with its own entry and tests. The parent stays
as the group's heading (its references stay valid) but isn't itself a check: its result is the children's roll-up
(✓ only when every child is ✓ or covered by an automated test alone, ✗ if any is ✗, ◐ if any has a result, else ☐).
A test should name the sub-rules it checks; one still naming a split parent is reported (re-point it).

Every radio or bench entry also has a How: line, right after Method:, saying who can run the radio step:

    How: auto-radio — what the PC script does     (remote keys + UI readback + the terminal and link tools)
    How: auto-sdr — what it keys and measures     (PC keying on the tx-dry build + the RTL-SDR, tools/bench.py)
    How: host-covered — the tests, what to confirm (host/sim tests carry it; one quick batched pass on the radio)
    How: hands: NEED — what to do, what to look for (a person: a place, equipment, a physical action, judgement)

A hands entry also names its hand check, the one physical action that covers it, on a `Hand check: NAME` line;
the checks (session, minutes, setup, what to do) are in docs/test-plan/hand-checks.md. Several entries share a check
when the same action covers them (the emergency double PTT, switching on holding PTT, keying the clear radio...).

From these the tool also writes docs/test-by-hand.md (each hand check once, with the rules it covers and what to
look for in each, by session, the most rules per minute first) and docs/test-auto.md (what the PC will run, for
scripting). tools/record_hand.py records a check's result into every entry it covers.
"""
import argparse, pathlib, re, sys

HERE = pathlib.Path(__file__).resolve().parent
FERRITE = HERE.parent
WORK = FERRITE.parent
DOCS = WORK / "docs"

RULE = re.compile(r"^>\s+- \*\*([A-Z]+)-(\d+)(?:\.(\d+))?\*\*\s*(.*)$")
CHAPTER = re.compile(r"^## (?:(\d+[a-z]?)\. |Appendix ([A-Z])\. )(.*)$")


def slug(s):
    return re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")


def rule_key(rid):
    """Sort key for rule IDs: (prefix, number, sub-number), TX-30 before TX-30.1 before TX-31."""
    p, _, n = rid.partition("-")
    a, _, b = n.partition(".")
    return p, int(a) if a.isdigit() else 0, int(b) if b.isdigit() else -1


def entry_anchor(rid):
    """The GitHub anchor of an entry's heading (### TX-30.1 -> tx-301)."""
    return re.sub(r"[^a-z0-9-]", "", rid.lower())


# ---- the manual ----

def read_manual(path):
    """The book: the chapter files docs/manual/NN-*.md, A-*.md in order (path is the docs/manual.md index)."""
    d = pathlib.Path(path).with_suffix("")
    pat = re.compile(r"^(?:\d\d[a-z]?|[A-Z])-.*\.md$")
    return "".join(f.read_text() for f in sorted(d.iterdir()) if pat.match(f.name))


def parse_manual(path):
    rules, chapters, status = [], [], {}
    chap = None
    section = ""
    for line in read_manual(path).splitlines():
        m = CHAPTER.match(line)
        if m:
            num, app, title = m.groups()
            if num:
                key = f"{int(re.match(r'\d+', num).group()):02d}{num.lstrip('0123456789')}-{slug(title)}"
                label = f"{num}. {title}"
            else:
                key = f"appendix-{app.lower()}-{slug(title)}"
                label = f"Appendix {app}. {title}"
            chap = {"key": key, "label": label, "anchor": slug(line[3:])}
            chapters.append(chap)
            status[key] = []
            section = ""
            continue
        if line.startswith("## "):
            chap = None
            continue
        if chap is None:
            continue
        if line.startswith("### "):
            section = line[4:].strip()
            continue
        m = RULE.match(line)
        if m:
            p, n, k, text = m.groups()
            text = text.strip()
            retired = bool(re.match(r"\(?(Moved to|Retired|Dropped)", text, re.I))
            rid = f"{p}-{n}" + (f".{k}" if k else "")
            rules.append({"id": rid, "prefix": p, "num": int(n), "sub": int(k) if k else None,
                          "parent": f"{p}-{n}" if k else None, "text": text, "chap": chap["key"],
                          "retired": retired})
        elif section == "Status" and "bench" in line.lower() and line.startswith("- "):
            status[chap["key"]].append(line[2:].strip())
    return rules, chapters, status


# ---- the tests ----

def id_pattern(prefixes):
    alt = "|".join(sorted(prefixes, key=len, reverse=True))
    return re.compile(rf"\b({alt})-(\d+)(?:\.(\d+)\b)?(?:\s*(?:–|—|\.\.|to)\s*(?:({alt})-)?(\d+))?\b")


def ids_in(text, pat):
    out = set()
    for m in pat.finditer(text):
        p, a, k, p2, b = m.groups()
        if k is not None:  # a sub-rule (TX-30.1); no ranges of them
            out.add(f"{p}-{a}.{k}")
            if b is not None and p2:
                out.add(f"{p2}-{b}")
            continue
        a = int(a)
        if b is not None and (p2 is None or p2 == p) and int(b) >= a and int(b) - a <= 40:
            out.update(f"{p}-{k}" for k in range(a, int(b) + 1))
        else:
            out.add(f"{p}-{a}")
            if b is not None and p2:
                out.add(f"{p2}-{b}")
    return out


def scan_tests(roots, pat):
    tests = []
    for root in roots:
        if not root.exists():
            continue
        for f in sorted(root.rglob("*.rs")):
            if "target" in f.parts:
                continue
            lines = f.read_text(errors="replace").splitlines()
            for i, line in enumerate(lines):
                if not re.match(r"\s*#\[(?:test|tokio::test|async_test)\]", line):
                    continue
                # the doc comment and attributes above
                head = []
                j = i - 1
                while j >= 0 and re.match(r"\s*(///|#\[|//)", lines[j]):
                    head.append(lines[j])
                    j -= 1
                # the fn and its body, by braces
                k = i + 1
                while k < len(lines) and not re.search(r"\bfn\s+\w+", lines[k]):
                    head.append(lines[k])
                    k += 1
                if k >= len(lines):
                    continue
                name = re.search(r"\bfn\s+(\w+)", lines[k]).group(1)
                depth, body, started = 0, [], False
                for L in lines[k:]:
                    body.append(L)
                    depth += L.count("{") - L.count("}")
                    if "{" in L:
                        started = True
                    if started and depth <= 0:
                        break
                comments = "\n".join(head) + "\n" + "\n".join(
                    c for L in body for c in re.findall(r"//.*$", L))
                rel = f.relative_to(WORK)
                kind = "sim" if any(p in ("sim", "sim-web") for p in rel.parts[:2]) else "host"
                tests.append({"name": name, "file": str(rel), "line": k + 1, "kind": kind,
                              "ids": ids_in(comments + "\n" + name.upper().replace("_", "-"), pat)})
    return tests


# ---- the hand-kept entries ----

ENTRY = re.compile(r"^### ([A-Z]+-\d+(?:\.\d+)?)\s*$")


def parse_sources(d):
    entries = {}
    for f in sorted(d.glob("*.md")):
        cur = None
        in_results = False
        for line in f.read_text().splitlines():
            m = ENTRY.match(line)
            if m:
                cur = {"id": m.group(1), "file": f.name, "method": "", "results": []}
                entries[cur["id"]] = cur
                in_results = False
                continue
            if cur is None:
                continue
            if line.startswith("## ") or line.startswith("# "):
                cur = None
                continue
            m = re.match(r"^(Method|How|Hand check|Equipment|Steps|Pass):\s*(.*)$", line)
            if m:
                cur[m.group(1).lower().replace(" ", "_")] = m.group(2).strip()
                in_results = False
            elif line.startswith("Results:"):
                in_results = True
            elif in_results and line.startswith("|") and not re.match(r"^\|\s*(Date|---)", line):
                cells = [c.strip() for c in line.strip().strip("|").split("|")]
                if cells and cells[0]:
                    cur["results"].append(cells)
    return entries


# ---- drafts ----

BENCH = [
    (r"deviation", "service monitor, or an SDR with a deviation readout"),
    (r"desens|harmonic|spur|third", "signal generator, attenuators, dummy load, an SDR"),
    (r"\bmA\b|µA|current", "multimeter in series with the battery, or a bench supply with a current readout"),
    (r"\bW\b|watt|power level|dummy load|PA drive|DAC2|ramp", "dummy load and RF power meter"),
    (r"on a scope|oscilloscope|settle|edge|\b\d+ ?ms\b.*(key|unkey|relay)", "oscilloscope on the named pin"),
    (r"temperature|thermometer", "thermometer"),
    (r"sensitiv|dBm|\bdB\b", "signal generator and attenuators"),
    (r"battery|volt|\bV\b", "bench supply or multimeter"),
]
RADIO = re.compile(r"relay|\bPA\b|BK4829|SI4732|\bGPS\b|Bluetooth|BT513|battery|\bLED|backlight|keypad light|"
                   r"speaker|SPI flash|\bflash\b|bootloader|cable|USART|UART|\bADC|\bDAC|REG_|\bP[A-E]\d|"
                   r"\bmic\b|audio|antenna|\bPTT\b|current|power off|switch(es)? off|at boot|self-test|"
                   r"key test|side connector|charger|calibration|squelch|RSSI|carrier|on the air|transmit",
                   re.I)
BENCH_RE = re.compile(r"deviation|desens|harmonic|spur|\bmA\b|µA|dummy load|power meter|oscilloscope|"
                      r"on a scope|dBm|sensitiv|thermometer|ramp", re.I)
HW_PREFIX = {"SAFE", "PWR", "CAL", "BOOT", "SELF", "INST", "BACKUP", "UPD", "TX", "XB"}
REVIEW_RE = re.compile(r"\btask\b|\bcrate|compiled|\bcode\b|\bbuild\b|cargo feature|\bmodule\b|\bRAM\b|\bstack\b|"
                       r"interrupt|no_std|\bISR\b|\bowner\b|source of truth|library|libraries", re.I)
AIR_RE = re.compile(r"receiv|transmit|carrier|signal|on the air|squelch|repeater|tone|beacon|frame|packet|heard",
                    re.I)


def checks(text):
    """The rule's clauses as numbered checks (split on ';')."""
    body = re.sub(r"\s*\((?:decided|decision|settled)[^)]*\)", "", text, flags=re.I).strip().rstrip(".")
    parts, depth, cur = [], 0, ""
    for ch in body:
        depth += (ch in "([") - (ch in ")]")
        if ch == ";" and depth == 0:
            parts.append(cur.strip())
            cur = ""
        else:
            cur += ch
    parts = [p for p in parts + [cur.strip()] if p]
    if len(parts) == 1:
        return parts[0] + "."
    return " ".join(f"({i}) {p}." for i, p in enumerate(parts, 1))


def draft(rule, tests, bench_notes):
    rid, text = rule["id"], rule["text"]
    if rule["retired"]:
        return {"method": "n/a", "steps": "Nothing to test: the rule moved or was retired (see its text).",
                "pass": "n/a", "equipment": ""}
    auto = sorted({t["kind"] for t in tests})
    methods = list(auto)
    hw = bool(RADIO.search(text)) or rule["prefix"] in HW_PREFIX
    bench = bool(BENCH_RE.search(text))
    review = bool(REVIEW_RE.search(text)) or rule["prefix"] == "GOAL"
    if bench:
        methods.append("bench")
    elif review:
        methods.append("review")
    elif hw:
        methods.append("radio")
    elif not methods:
        methods.append("sim")
    equip = ""
    if "bench" in methods:
        for pat, what in BENCH:
            if re.search(pat, text, re.I):
                equip = what
                break
        equip = equip or "a second radio"
    elif "radio" in methods:
        equip = "the radio with Ferrite" + (", a second radio" if AIR_RE.search(text) else "")
    notes = [n for n in bench_notes if rid in ids_in(n, ID_PAT)]
    where = {"sim": "In the simulator (`cargo sim` or the web sim)", "radio": "On the radio",
             "bench": "On the bench", "review": "Code review"}
    steps = []
    if auto:
        steps.append("Automated: " + ", ".join(f"`{t['name']}`" for t in tests[:4])
                     + (f" (+{len(tests) - 4})" if len(tests) > 4 else "") + ".")
    for m in ("sim", "radio", "bench", "review"):
        if m in methods and m not in auto:
            if m == "review":
                steps.append("Code review: find where the code does this and confirm it holds.")
            else:
                watch = {"bench": "the screen, the cable log and the instruments", "radio": "the screen and the "
                         "cable log", "sim": "the screen"}[m]
                steps.append(f"{where[m]}: set up each case and watch {watch}.")
    for n in notes[:2]:
        steps.append("Chapter bench check: " + n.rstrip(".") + ".")
    if rule["prefix"] in {"KEY", "SAFE", "PWR"} and "radio" in methods:
        steps.append("See also `docs/hwtest-checklist.md`.")
    if rule["prefix"] in {"BOOT", "INST", "BACKUP", "SELF", "UPD"} and "radio" in methods:
        steps.append("See also `docs/bringup.md`.")
    steps.append("Check: " + checks(text))
    return {"method": ", ".join(methods), "steps": " ".join(steps),
            "pass": "Every check holds, and nothing the rule doesn't name changes.", "equipment": equip}


def write_entry(e, rid, manual_entry):
    out = [f"### {rid}", f"Method: {e['method']}"]
    if e["equipment"]:
        out.append(f"Equipment: {e['equipment']}")
    out += [f"Steps: {e['steps']}", f"Pass: {e['pass']}"]
    if manual_entry:
        out += ["Results:", "| Date | Firmware | Result | Notes |", "| --- | --- | --- | --- |", "| | | ☐ | |"]
    return "\n".join(out) + "\n"


# ---- output ----

def cell(s, n=110):
    s = s.replace("|", "\\|").replace("\n", " ")
    return s if len(s) <= n else s[: n - 1].rstrip() + "…"


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--docs", type=pathlib.Path, default=DOCS)
    ap.add_argument("--init", action="store_true", help="add draft entries for rules without one")
    ap.add_argument("--coverage", action="store_true", help="short report; exit 1 on gaps or stale tags")
    ap.add_argument("--parents", action="store_true", help="with --coverage: list the tests still naming a split parent")
    a = ap.parse_args()

    rules, chapters, status = parse_manual(a.docs / "manual.md")
    by_id = {r["id"]: r for r in rules}
    seen, dups = set(), []
    for r in rules:
        if r["id"] in seen:
            dups.append(r["id"])
        seen.add(r["id"])
    global ID_PAT
    ID_PAT = id_pattern({r["prefix"] for r in rules})
    # ferrite/digimodes holds the decoders since Oct 23, 2026 (the source from then on); udigimodes is the parked
    # library, still scanned for the rules its own tests name.
    roots = [FERRITE / "crates", FERRITE / "sim", FERRITE / "sim-web", FERRITE / "firmware", FERRITE / "drivers",
             FERRITE / "digimodes", WORK / "udigimodes" / "crates"]
    tests = scan_tests(roots, ID_PAT)
    covered = {}
    for t in tests:
        for i in t["ids"]:
            covered.setdefault(i, []).append(t)
    stale = sorted({i for i in covered if i not in by_id}, key=rule_key)
    kids = children(rules)
    # tests that still name a split parent (each should name the sub-rules it checks)
    parent_cites = sorted({(i, t["file"], t["line"], t["name"]) for i in covered if i in kids for t in covered[i]},
                          key=lambda x: (rule_key(x[0]), x[1], x[2]))
    src = a.docs / "test-plan"
    src.mkdir(exist_ok=True)

    if a.init:
        entries = parse_sources(src)
        added = 0
        for ch in chapters:
            missing = [r for r in rules if r["chap"] == ch["key"] and r["id"] not in entries and r["id"] not in kids]
            if not missing:
                continue
            f = src / f"{ch['key']}.md"
            if not f.exists():
                notes = status.get(ch["key"], [])
                head = [f"# Test plan: {ch['label']}", "",
                        f"Entries for the rules of [manual chapter {ch['label']}](../manual.html#{ch['anchor']}). "
                        "Drafts were generated by `ferrite/tools/testplan.py --init` from the rule text; refine "
                        "the steps and pass criteria as the feature is tested. Record radio and bench results "
                        "in each entry's Results table (date, firmware commit, pass or fail, notes); "
                        "`docs/test-plan.md` is regenerated from these files.", ""]
                if notes:
                    head += ["Bench checks from the chapter's Status:", ""] + [f"- {n}" for n in notes] + [""]
                f.write_text("\n".join(head))
            with f.open("a") as fh:
                for r in missing:
                    e = draft(r, covered.get(r["id"], []), status.get(ch["key"], []))
                    manual = any(m in e["method"] for m in ("radio", "bench", "review"))
                    fh.write("\n" + write_entry(e, r["id"], manual))
                    added += 1
        print(f"added {added} draft entries to {src}")

    entries = parse_sources(src)
    live = [r for r in rules if not r["retired"] and r["id"] not in kids]  # the checks: a split parent isn't one
    auto = [r for r in live if r["id"] in covered]
    none = [r for r in rules if r["id"] not in covered and r["id"] not in entries and r["id"] not in kids]
    status_of = {r["id"]: rule_status(r["id"], entries, covered) for r in rules if r["id"] not in kids}
    for pid in kids:
        status_of[pid] = roll_up([status_of.get(k, "") for k in kids[pid]])
    methods = {}
    for r in rules:
        for m in [x.strip() for x in entries.get(r["id"], {}).get("method", "").split(",") if x.strip()]:
            methods[m] = methods.get(m, 0) + 1
    pct = 100 * len(auto) / max(len(live), 1)

    if a.coverage:
        print(f"rules: {len(rules)} ({len(live)} live), automated: {len(auto)} ({pct:.0f}%), "
              f"tests scanned: {len(tests)}")
        print("by method (entries): " + ", ".join(f"{k} {v}" for k, v in sorted(methods.items())))
        if none:
            print(f"no test and no entry ({len(none)}): " + " ".join(r["id"] for r in none))
        if stale:
            print(f"tests cite rules the manual doesn't have ({len(stale)}): "
                  + " ".join(f"{i} ({covered[i][0]['file']}:{covered[i][0]['line']})" for i in stale))
        if dups:
            print(f"rule IDs used twice in the manual ({len(dups)}): " + " ".join(dups))
        if kids:
            print(f"split rules: {len(kids)} parents with {sum(len(v) for v in kids.values())} sub-rules")
        if parent_cites:
            # the count only (the list is 30 KB and long-standing): --parents lists them
            print(f"tests that still name a split parent: {len(parent_cites)} (name the sub-rules instead; --parents lists them)")
            if a.parents:
                print(" ".join(f"{i} ({f.split('/')[-1]}:{n} {t})" for i, f, n, t in parent_cites))
        sys.exit(1 if none or stale or dups else 0)

    checks = [r for r in live if needs_radio(entries.get(r["id"], {}))]
    tally = {m: sum(1 for r in checks if status_of[r["id"]] == m) for m in ("✓", "◐", "✗", "☐")}
    tally["☐"] += sum(1 for r in checks if status_of[r["id"]] not in ("✓", "◐", "✗", "☐"))
    groups = {m: sum(1 for p in kids if status_of[p] == m) for m in ("✓", "◐", "✗", "☐")}
    out = ["# Ferrite test plan", "",
           "Generated by `ferrite/tools/testplan.py` from [the manual](manual.md)'s numbered rules, the tests in "
           "ferrite and udigimodes (a test covers a rule when the rule's ID is in its doc comment, name "
           "or body comments), and the hand-kept entries in [`test-plan/`](test-plan/). Don't edit this file: "
           "edit the entries, then run the tool.", "",
           "## Summary", "",
           f"- Rules: {len(rules)} ({len(live)} live, {len(rules) - len(live)} moved or retired).",
           f"- Covered by an automated test: {len(auto)} of {len(live)} live rules ({pct:.0f}%), "
           f"from {len(tests)} tests scanned.",
           "- Entries by method: " + ", ".join(f"{k} {v}" for k, v in sorted(methods.items())) + ".",
           f"- Rules with neither a test nor an entry: {len(none)}"
           + (": " + " ".join(r["id"] for r in none) if none else "") + ".",
           f"- Radio and bench checks by their latest result: ✓ {tally['✓']}, ◐ {tally['◐']}, ✗ {tally['✗']}, "
           f"☐ {tally['☐']} (of {len(checks)}).",
           f"- Split rules (decision 160): {len(kids)} parents with {sum(len(v) for v in kids.values())} sub-rules, "
           f"each sub-rule one check; a parent is its sub-rules' roll-up: ✓ {groups['✓']}, ◐ {groups['◐']}, "
           f"✗ {groups['✗']}, ☐ {groups['☐']}."
           + (f" Tests still naming a split parent: {len(parent_cites)}: "
              + ", ".join(f"{i} (`{t}`)" for i, f, n, t in parent_cites) if parent_cites else ""),
           f"- Rule IDs used twice in the manual: {len(dups)}" + (": " + " ".join(dups) if dups else "") + ".",
           f"- Rules cited by tests that the manual doesn't have: {len(stale)}"
           + (": " + ", ".join(f"{i} (`{covered[i][0]['file']}:{covered[i][0]['line']}`)" for i in stale)
              if stale else "") + "."]
    done = sum(1 for e in entries.values() if e["results"])
    out += [f"- Entries with a recorded radio, bench or review result: {done}.", ""]
    for ch in chapters:
        rs = [r for r in rules if r["chap"] == ch["key"]]
        if not rs:
            continue
        n_auto = sum(1 for r in rs if r["id"] in covered and r["id"] not in kids)
        out += [f"## {ch['label']}", "",
                f"{len(rs)} rules, {n_auto} automated. Entries: [`test-plan/{ch['key']}.md`]"
                f"(test-plan/{ch['key']}.md).", "",
                "| Rule | What | Method | Automated by | Manual steps and pass | Result |",
                "| --- | --- | --- | --- | --- | --- |"]
        for r in rs:
            e = entries.get(r["id"], {})
            ts = covered.get(r["id"], [])
            by = ", ".join(f"`{t['name']}`" for t in ts[:3]) + (f" +{len(ts) - 3}" if len(ts) > 3 else "")
            steps = e.get("steps", "")
            if steps.startswith("Automated:"):
                steps = ""
            link = f"[steps](test-plan/{e['file']}#{entry_anchor(r['id'])})" if e and steps else ""
            if r["id"] in kids:
                result = f"{status_of[r['id']]} (its {len(kids[r['id']])} sub-rules)"
                method = "split"
                link = ""
            else:
                res = e.get("results", [])
                if res:
                    last = res[-1]
                    result = f"{last[2] if len(last) > 2 else ''} {last[0]}".strip()
                elif any(m in e.get("method", "") for m in ("radio", "bench", "review")):
                    result = "☐"
                elif ts:
                    result = "auto"
                else:
                    result = ""
                method = e.get("method", "")
            what = cell(r["text"])
            label = f"↳ [{r['id']}](manual.html#{r['id']})" if r["parent"] else f"[{r['id']}](manual.html#{r['id']})"
            out.append(f"| {label} | {what} | {method} | {by} | {link} | {result} |")
        out.append("")
    hows, no_how = how_counts(rules, entries)
    out.insert(out.index("## Summary") + 2,
               "- Radio and bench entries by How: " + ", ".join(f"{k} {v}" for k, v in hows.items())
               + (f"; without a How: line {len(no_how)}: " + " ".join(no_how) if no_how else "")
               + ". By-hand checklist: [test-by-hand.md](test-by-hand.md); for the PC: [test-auto.md](test-auto.md).")
    (a.docs / "test-plan.md").write_text("\n".join(out))
    n_checks, n_hand, minutes, unknown, unused, without = write_by_hand(a.docs, rules, entries)
    write_auto(a.docs, rules, entries, covered)
    print(f"by hand: {n_checks} checks covering {n_hand} rules, about {minutes} min"
          + (f"; unknown hand checks {unknown}" if unknown else "")
          + (f"; hand checks nothing uses {unused}" if unused else "")
          + (f"; hands entries without a Hand check {without}" if without else ""))
    print("by How: " + ", ".join(f"{k} {v}" for k, v in hows.items())
          + (f", without How {len(no_how)}" if no_how else ""))
    print(f"wrote {a.docs / 'test-plan.md'}: {len(rules)} rules, {len(auto)} automated ({pct:.0f}%), "
          f"{len(none)} without test or entry, {len(stale)} stale tags"
          + (f", {len(dups)} rule IDs used twice: {' '.join(dups)}" if dups else ""))


# ---- Split rules: the parents and their roll-up ----

def children(rules):
    """{parent id: [sub-rule ids]} for the rules split into sub-rules."""
    kids = {}
    for r in rules:
        if r.get("parent"):
            kids.setdefault(r["parent"], []).append(r["id"])
    return kids


def rule_status(rid, entries, covered):
    """A leaf rule's status: ✓ ◐ ✗ from its latest radio/bench/review result, ☐ when one is due and missing,
    "auto" when automated tests alone carry it, "" when nothing does."""
    e = entries.get(rid, {})
    lr = last_result(e)
    if lr in ("✓", "◐", "✗"):
        return lr
    if lr or any(m in e.get("method", "") for m in ("radio", "bench", "review")):
        return "☐"
    return "auto" if rid in covered else ""


def roll_up(marks):
    """A split parent's status from its sub-rules': ✗ if any ✗; ✓ when every one is ✓ (or automated alone);
    ◐ if any has a result; else ☐."""
    if any(m == "✗" for m in marks):
        return "✗"
    if marks and all(m in ("✓", "auto") for m in marks):
        return "✓" if "✓" in marks else "auto"
    if any(m in ("✓", "◐") for m in marks):
        return "◐"
    return "☐"


# ---- How: the by-hand list and the PC's list ----

HOWS = ("auto-radio", "auto-sdr", "host-covered", "hands")

# The hands sessions: (key, title, kind). A hand check names its session (docs/test-plan/hand-checks.md). The bench
# sessions come first, then those needing equipment, each kind ordered by the rules still to do per minute; the
# places and times last, in this order.
SESSIONS = [
    ("bench", "At the bench: the radio, the cable and the SDR (tx-dry)", "bench"),
    ("mic", "At the bench with the speaker mic", "bench"),
    ("clear-rx", "At the bench with the clear radio: receiving", "bench"),
    ("clear-more", "At the bench with the clear radio: scanning, crossband and data", "bench"),
    ("supply", "With the bench supply and a meter", "equipment"),
    ("load", "With the tx build into a dummy load and power meter", "equipment"),
    ("scope", "With an oscilloscope", "equipment"),
    ("siggen", "With a signal generator", "equipment"),
    ("chrome", "On the Mac in Chrome", "equipment"),
    ("sky", "GPS under the sky, a walk and a drive", "place"),
    ("restore", "Restore day (back to stock and return)", "place"),
    ("same", "When a real SAME test comes (weekly, no fixed time)", "place"),
]
HAND_CHECKS = "hand-checks.md"
CHECK_HEAD = re.compile(r"^### ([A-Z][A-Z0-9-]*)\s*$")


def parse_hand_checks(path):
    """{name: {session, minutes, unattended, setup, do}} from docs/test-plan/hand-checks.md, in file order."""
    out = {}
    if not path.exists():
        return out
    cur = None
    for line in path.read_text().splitlines():
        m = CHECK_HEAD.match(line)
        if m and not ENTRY.match(line):
            cur = out.setdefault(m.group(1), {"name": m.group(1), "session": "", "minutes": 0, "unattended": 0,
                                              "setup": "", "do": "", "release": "0.9"})
            continue
        if line.startswith("#"):
            cur = None
            continue
        m = re.match(r"^(Session|Minutes|Unattended|Setup|Do|Release):\s*(.*)$", line)
        if cur is not None and m:
            k, v = m.group(1).lower(), m.group(2).strip()
            cur[k] = int(v) if k in ("minutes", "unattended") and v.isdigit() else v
    return out


def hand_items(rules, entries):
    """[(rule, entry, need, what)] for the hands entries, in manual order."""
    out = []
    for r in rules:
        e = entries.get(r["id"])
        if not e or not needs_radio(e):
            continue
        k, rest = how_of(e)
        if k != "hands":
            continue
        need, _, what = rest.partition(" — ")
        out.append((r, e, need.strip(), (what or rest).strip()))
    return out


def hand_plan(docs, rules, entries):
    """The hand checks with their rules: (checks {name: check + "items"}, unknown [(rule, name)], unused [name],
    without [rule ids]). A hands entry without a Hand check line is its own check, in the "other" session."""
    checks = parse_hand_checks(docs / "test-plan" / HAND_CHECKS)
    for c in checks.values():
        c["items"] = []
    unknown, without = [], []
    for item in hand_items(rules, entries):
        r, e, need, what = item
        name = e.get("hand_check", "")
        if name and name in checks:
            checks[name]["items"].append(item)
            continue
        (unknown.append((r["id"], name)) if name else without.append(r["id"]))
        checks[r["id"]] = {"name": r["id"], "session": "other", "minutes": 5, "unattended": 0,
                           "setup": need, "do": what, "items": [item]}
    unused = [n for n, c in checks.items() if not c["items"]]
    return {n: c for n, c in checks.items() if c["items"]}, unknown, unused, without


def how_of(e):
    h = e.get("how", "")
    for k in HOWS:
        if h.startswith(k):
            rest = h[len(k):].lstrip(" :—-").strip()
            return k, rest
    return "", h


def needs_radio(e):
    return any(m.strip() in ("radio", "bench") for m in e.get("method", "").split(","))


def how_counts(rules, entries):
    counts = {k: 0 for k in HOWS}
    missing = []
    for r in rules:
        e = entries.get(r["id"])
        if not e or not needs_radio(e):
            continue
        k, _ = how_of(e)
        if k:
            counts[k] += 1
        else:
            missing.append(r["id"])
    return counts, missing


def last_result(e):
    res = [x for x in e.get("results", []) if x and x[0]]
    if not res:
        return ""
    last = res[-1]
    mark = (last[2] if len(last) > 2 else "").strip()
    return {"pass": "✓", "partial": "◐", "fail": "✗"}.get(mark, mark)


def n_of(n, word):
    return f"{n} {word}" + ("" if n == 1 else "s")


def write_by_hand(docs, rules, entries):
    checks, unknown, unused, without = hand_plan(docs, rules, entries)
    titles = {k: (t, kind) for k, t, kind in SESSIONS}
    by_session = {}
    for c in checks.values():
        by_session.setdefault(c["session"] if c["session"] in titles else "other", []).append(c)

    def todo(c):
        return [i for i in c["items"] if last_result(i[1]) != "✓"]

    def mins(cs):
        return sum(c["minutes"] for c in cs if todo(c))

    def rate(key):
        cs = by_session[key]
        n = sum(len(todo(c)) for c in cs)
        return n / max(mins(cs), 1)

    order = []
    for kind in ("bench", "equipment"):
        keys = [k for k, _, kd in SESSIONS if kd == kind and k in by_session]
        order += sorted(keys, key=lambda k: (-rate(k), [s[0] for s in SESSIONS].index(k)))
    order += [k for k, _, kd in SESSIONS if kd == "place" and k in by_session]
    order += ["other"] if "other" in by_session else []
    n_rules = sum(len(c["items"]) for c in checks.values())
    n_todo = sum(len(todo(c)) for c in checks.values())
    live = [c for c in checks.values() if todo(c)]
    total = mins(live)
    unatt = sum(c["unattended"] for c in live)
    out = ["# Testing by hand", "",
           "Generated by `ferrite/tools/testplan.py` from the `How: hands` entries in [`test-plan/`](test-plan/) and "
           "their hand checks ([`test-plan/hand-checks.md`](test-plan/hand-checks.md)). Don't edit this file: edit "
           "those, then run the tool. These are the radio checks that need a person (a place, equipment, a physical "
           "action, the clear radio, eyes or ears); everything else the PC runs ([test-auto.md](test-auto.md)). Each "
           "check is one action done once, with every rule it covers looked at during it. Record it once for all of "
           "them: `tools/record_hand.py CHECK ✓ \"note\"` (a rule that differs: add `RULE=✗:\"why\"`; one rule "
           "alone: `tools/record_hand.py RULE ◐ \"note\"`), or edit the entries' Results tables by hand.", "",
           f"{len(checks)} checks covering {n_rules} rules ({n_todo} without a pass yet), in "
           f"{sum(1 for k in order if any(todo(c) for c in by_session[k]))} sessions: about {total} min of a "
           f"person's time" + (f", plus {unatt} min the radio sits unattended" if unatt else "") + ".", ""]
    if unknown or without or unused:
        out += ["Plan problems: "
                + "; ".join(x for x in (
                    "unknown hand checks: " + ", ".join(f"{r} → {n}" for r, n in unknown) if unknown else "",
                    "hands entries without a Hand check: " + ", ".join(without) if without else "",
                    "hand checks nothing points at: " + ", ".join(unused) if unused else "") if x) + ".", ""]
    # The checks that protect alpha testers come first (Release: 0.8 in hand-checks.md; Alex, Oct 27); the rest are
    # before 0.9. Sessions keep their order below; each 0.8 check is marked there too.
    sess_no = {k: i for i, k in enumerate(order, 1)}
    alpha = [c for c in checks.values() if c.get("release") == "0.8"]
    if alpha:
        a_live = [c for c in alpha if todo(c)]
        out += [f"## For the 0.8 alpha: {n_of(len(a_live), 'check')} left of {len(alpha)}, about {mins(a_live)} min", "",
                "The checks that protect alpha testers (transmit refusals and limits, switching off and on, recovery, "
                "the low-battery cut-off, install and back to stock). Everything else below is before 0.9.", "",
                "| Check | Session | Rules to do | Minutes | Done |", "| --- | --- | --- | --- | --- |"]
        for c in alpha:
            k = c["session"] if c["session"] in titles else "other"
            out.append(f"| {c['name']} | [{titles.get(k, ('Other', ''))[0]}](#session-{sess_no.get(k, '')}) | "
                       f"{len(todo(c))} | {c['minutes']} | {'✓' if not todo(c) else ''} |")
        out.append("")
    out += ["## Every session", "", "| # | Session | Checks | Rules to do | Minutes |", "| --- | --- | --- | --- | --- |"]
    for i, k in enumerate(order, 1):
        cs = by_session[k]
        title = titles.get(k, ("Other (no hand check)", ""))[0]
        un = sum(c["unattended"] for c in cs if todo(c))
        out.append(f"| {i} | [{title}](#session-{i}) | {sum(1 for c in cs if todo(c))} of {len(cs)} | "
                   f"{sum(len(todo(c)) for c in cs)} | {mins(cs)}" + (f" (+{un} unattended)" if un else "") + " |")
    out.append("")
    for i, k in enumerate(order, 1):
        cs = by_session[k]
        title = titles.get(k, ("Other (no hand check)", ""))[0]
        live = [c for c in cs if todo(c)]
        out += [f'<a id="session-{i}"></a>', "",
                f"## {i}. {title}: {n_of(len(live), 'check')}, {n_of(sum(len(todo(c)) for c in live), 'rule')}, "
                f"about {mins(cs)} min",
                ""]
        for c in live:
            t = todo(c)
            done = [x for x in c["items"] if x not in t]
            out += [f"### {c['name']}" + (" (0.8)" if c.get("release") == "0.8" else "") + f": {n_of(len(t), 'rule')}"
                    + (f" (+{len(done)} passed)" if done else "")
                    + f", {c['minutes']} min"
                    + (f" (+{c['unattended']} unattended)" if c["unattended"] else ""), ""]
            if c["setup"]:
                out += [f"Setup: {c['setup']}", ""]
            if c["do"]:
                out += [f"Do: {c['do']}", ""]
            out.append("Look for:")
            out.append("")
            for r, e, need, what in c["items"]:
                lr = last_result(e)
                box = "- [x]" if lr == "✓" else "- [ ]"
                out.append(f"{box} [{r['id']}](manual.html#{r['id']}): {what}"
                           + (" (passed: rechecked on the way)" if lr == "✓" else f" (last: {lr})" if lr else ""))
            out += ["", f"Record: `tools/record_hand.py {c['name']} ✓ \"note\"`", ""]
        passed = [c for c in cs if not todo(c)]
        if passed:
            out += ["Already passed (recheck only after a change): "
                    + ", ".join(f"{c['name']} (" + ", ".join(f"[{r['id']}](manual.html#{r['id']})"
                                                           for r, *_ in c["items"]) + ")" for c in passed) + ".", ""]
    (docs / "test-by-hand.md").write_text("\n".join(out))
    return len(checks), n_rules, total, unknown, unused, without


def scan_uitests(d=HERE / "uitests"):
    """The on-radio UI tests (tools/uitests/*.py): {rule: [(test name, known deviation or "")]}."""
    out = {}
    for f in sorted(d.glob("*.py")):
        src = f.read_text()
        for m in re.finditer(r"^@test\((.*?)\)\s*\ndef (\w+)", src, re.S | re.M):
            args, name = m.groups()
            known = re.search(r"xfail=", args)
            where = re.search(r'where="(\w+)"', args)
            for rid in re.findall(r'"([A-Z]+-\d+(?:\.\d+)?)"', args):
                out.setdefault(rid, []).append((f"{f.stem}.{name}", "known" if known else (where.group(1) if where else "")))
    return out


def write_auto(docs, rules, entries, covered):
    ui = scan_uitests()
    for rid, ts in scan_uitests(HERE / "tworadio_tests").items():  # tools/tworadio.py: two radios, one keyed by the PC
        ui.setdefault(rid, []).extend((f"tworadio:{n.split('.', 1)[-1]}", k) for n, k in ts)
    out = ["# Radio checks the PC runs", "",
           "Generated by `ferrite/tools/testplan.py` from the `How:` lines in [`test-plan/`](test-plan/). "
           "Don't edit this file. **auto-radio**: a PC script with remote keys and UI readback (LINK-19), the "
           "terminal and link tools (`slot`, `set`/`get`, `bk`, `gpio`, `mem`, `gps`, restart, the cable log) "
           "checks it unattended. **auto-sdr**: PC keying on the `tx-dry` build (LINK-20) and the RTL-SDR "
           "(`tools/bench.py`, `sdrtx.py`, `sdrtones.py`, `sdrsuite.py`). **host-covered**: the host and sim "
           "tests carry the logic; the radio step is one quick confirmation pass. The by-hand rest: "
           "[test-by-hand.md](test-by-hand.md). **UI test**: the scripted check in `tools/uitests/` "
           "(`tools/uitest.py`; *known* marks a test of a known deviation, *radio* one the simulator can't run), or "
           "*tworadio:NAME* in `tools/tworadio_tests/` (`tools/tworadio.py`: two tx-dry radios, the PC keying one and "
           "reading the other back).", ""]
    for k, title in (("auto-radio", "auto-radio"), ("auto-sdr", "auto-sdr"),
                     ("host-covered", "host-covered (one batched confirmation pass)")):
        rows = []
        for r in rules:
            e = entries.get(r["id"])
            if not e or not needs_radio(e):
                continue
            kk, rest = how_of(e)
            if kk == k:
                rows.append((r, e, rest))
        todo = sum(1 for r, e, _ in rows if last_result(e) != "✓")
        scripted = sum(1 for r, _, _ in rows if r["id"] in ui)
        out += [f"## {title}: {len(rows)} ({todo} without a pass yet, {scripted} with a UI test)", "",
                "| Rule | What the PC does | Tests | UI test | Last |", "| --- | --- | --- | --- | --- |"]
        for r, e, rest in rows:
            ts = covered.get(r["id"], [])
            by = ", ".join(f"`{t['name']}`" for t in ts[:2]) + (f" +{len(ts) - 2}" if len(ts) > 2 else "")
            us = ui.get(r["id"], [])
            script = ", ".join(f"`{n}`" + (f" ({k})" if k else "") for n, k in us[:2]) + (f" +{len(us) - 2}" if len(us) > 2 else "")
            out.append(f"| [{r['id']}](manual.html#{r['id']}) | {cell(rest, 160)} | {by} | {script} | {last_result(e)} |")
        out.append("")
    others = sorted((rid for rid in ui if rid not in {r["id"] for r in rules if (entries.get(r["id"]) or {}) and
                                                         how_of(entries[r["id"]])[0] in ("auto-radio", "auto-sdr", "host-covered")}),
                    key=rule_key)
    if others:
        out += [f"## Also checked by a UI test: {len(others)}", "",
                "Rules whose entry isn't an auto entry above (host, sim or hands) that `tools/uitest.py` checks on the radio too.", "",
                "| Rule | UI test |", "| --- | --- |"]
        for rid in others:
            out.append(f"| [{rid}](manual.html#{rid}) | " + ", ".join(f"`{n}`" + (f" ({k})" if k else "") for n, k in ui[rid][:3]) + " |")
        out.append("")
    (docs / "test-auto.md").write_text("\n".join(out))


# Recording radio results into the entries: tools/record_results.py (uitest.py --record, bench.py --record).


ID_PAT = None

if __name__ == "__main__":
    main()
