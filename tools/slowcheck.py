#!/usr/bin/env python3
"""Check the fast/slow flash split of a firmware ELF (firmware/memory.x).

    python3 tools/slowcheck.py [target/thumbv7em-none-eabihf/release/ferrite] [-v]

Prints the region sizes and the fast budget, then checks interrupt context: every interrupt handler in the vector
table plus the tasks on the interrupt executors (Radio, DSP, input: their poll functions are reached only through
task pointers stored at spawn, so they are added as roots by name), followed through direct calls and through the
function pointers their code loads (closures, vtables, tables of handlers).

It fails (exit status 1) when interrupt context:
  - reaches a function in .slow_text that is not on SLOW_CALLS below, or
  - reads a constant in .slow_rodata that is not text (a table: lookup values, pointers, switch targets), unless
    the reading function is on SLOW_TABLES below.
Text in .slow_rodata read from interrupt context (log lines, error messages; told apart by its first 16 bytes being
printable) is fine and only counted: it is read once per message, not per sample. A hot table must be a named `static` (its own `.rodata.<symbol>` section stays
fast); an unnamed `const`/literal goes to the slow part (memory.x). -v lists every slow read.

Also listed: fast functions that can call into .slow_text (only Reset/main, the thread executor and the UI task
should) and fast data holding pointers into .slow_text. Needs arm-none-eabi-objdump / readelf / nm on the PATH.
"""
import bisect, collections, re, struct, subprocess, sys

# Slow code that interrupt context calls on purpose, each with its reason (memory.x has the same list with the
# rules). A name matches when it contains the pattern (demangled).
SLOW_CALLS = {
    "Radio>::fm_poll": "FM broadcast report, at most ten times a second (BCR-2)",
    "Radio>::bcast_off": "FM broadcast watch mask, on a mode change (BCR-23)",
    "Radio>::scope_tick": "band scope / home strip step, once a point, 2 ms apart at least (SCOPE-4, MODE-5)",
    "tonehint": "tone hint step, at most once a receive tick (TSRCH-12, -13)",
    "chip::heard_tone": "heard-tone decode for the tone hint (TSRCH-12)",
    "gpspower": "GPS power decision, a few times a second (GPS-26 to -33)",
    "store::part_read": "the SSB patch read from SPI, once per switch to a sideband (HF-19)",
}
# Slow tables that interrupt context reads on purpose: none of them per sample or per tick. Each entry is (regex,
# (min, max) size or None, reason). The regex is matched against the reading function and the functions inlined at
# the instruction that loads the table (demangled).
SLOW_TABLES = [
    (r"si4732::Si4732<", None, "SI4732 command bytes: sent over the bit-banged I2C bus, far slower than the flash"),
    (r"ferrite_core::tonegen::", None, "beep gains and tone sequences: read once per setting or once per tone step"),
    (r">::end_tone\b", None, "the end tone's step list: once per unkey"),
    (r"ferrite_core::txplan::", None, "the transmit plan's band tables: once per key-up"),
    (r"uafsk::fsk::Fsk<.*>::new\b", None, "the AFSK demodulator's set-up: once when a decoder starts"),
    (r"upocsag::demod::Receiver.*>::(new|batch)\b", None, "POCSAG set-up, and the baud of a batch: once per batch"),
    (r"radio::start_chips", None, "BK4829 start-up registers (bk4829's init): once at power-on"),
    # The Radio task's own body is its start-up (the Radio struct's first state) and the wait loop; the per-tick work
    # is in Radio::tick / cmd / run, checked like everything else. Small tables only: a large one is a real table.
    # The same body when the compiler leaves it in the task's inner closure instead of inlining it into the poll
    # (Nov 12, 2026: an 11-byte state table moved there with the bench build's TERM-53 change).
    (r"^<embassy_executor::raw::TaskStorage<ferrite::tasks::radio::|^ferrite::tasks::radio::__task_task::__task_task_inner_function::\{closure#0\}$", (0, 128),
     "the Radio task's start-up state: once at power-on"),
]
# Constant tables that hold numbers only: never scanned for function pointers, since two neighbouring samples can
# happen to look like one (Oct 24, 2026: two of udsp's sine values read as uaprs::build::message's address, a false
# "slow call" from the DSP task that came and went with the layout). Oct 27, 2026: stuck::NAMES' bytes "TT", 4, 8
# (the side PTT bit and its length) read as a .slow_rodata address once the txeq merge grew that part, a false slow
# table; txtest's pulse-shape taps, a false pointer into .slow_text. Nov 25, 2026: the SSTV test card's glyph rows,
# a false 2 KB slow table once ACARS's receiver moved the layout.
NUMBER_TABLES = re.compile(r"^(udsp::trig::(SIN|COS)|ferrite_core::stuck::NAMES|ferrite_core::txtest::(RRC|GAUSS)|ferrite_core::sstv::GLYPHS)\b")

# The tasks run by the interrupt executors (main.rs).
IRQ_TASKS = re.compile(r"TaskStorage.*7ferrite5tasks(5radio|3dsp|5input)11___task_task.*4poll")

def run(*a):
    return subprocess.run(a, capture_output=True, text=True, check=True).stdout

def main(elf, verbose):
    sec = {}
    for l in run("arm-none-eabi-readelf", "-SW", elf).splitlines():
        m = re.search(r"\]\s+(\S+)\s+\S+\s+([0-9a-f]+)\s+([0-9a-f]+)\s+([0-9a-f]+)", l)
        if m:
            sec[m.group(1)] = tuple(int(m.group(i), 16) for i in (2, 3, 4))
    inside = lambda a, s: s in sec and sec[s][0] <= a < sec[s][0] + sec[s][2]
    lo, _, n = sec[".slow_text"]
    hi = lo + n
    fast_end = max(sec[s][0] + sec[s][2] for s in (".rodata", ".fast_ui"))
    gaps = sum(sec[s][2] for s in (".fast_gap1", ".fast_gap2") if s in sec)
    print(f"fast  0x08003000-0x{fast_end:08x}: {fast_end - 0x08003000} B (.text {sec['.text'][2]}, .rodata {sec['.rodata'][2]}, "
          f".fast_ui {sec['.fast_ui'][2]}, by the tags {gaps})")
    nm = run("arm-none-eabi-nm", elf).splitlines()
    budget = next((int(l.split()[0], 16) for l in nm if l.endswith(" __fast_budget")), None)
    if budget is not None:
        print(f"fast budget {budget} B: {fast_end - 0x08003000} used, {budget - (fast_end - 0x08003000)} left")
    print(f"slow  .slow_rodata {sec['.slow_rodata'][2]} B, .slow_text {n} B")

    # Demangled names: nm and nm -C list the same symbols in the same order.
    dem = {}
    for a, b in zip(nm, run("arm-none-eabi-nm", "-C", elf).splitlines()):
        pa, pb = a.split(" ", 2), b.split(" ", 2)
        if len(pa) == 3 and len(pb) == 3:
            dem[pa[2]] = pb[2]
    d = lambda f: dem.get(f, f)

    code = [".text", ".slow_text"] + [s for s in (".fast_gap1", ".fast_gap2", ".fast_ui") if s in sec]
    dis = run("arm-none-eabi-objdump", "-d", "--no-show-raw-insn", *[x for s in code for x in ("-j", s)], elf)
    start, edges, lits, cur = {}, collections.defaultdict(set), collections.defaultdict(set), None
    pool, pc_loads = {}, []
    # A tbb/tbh's table follows it as data (objdump shows its offset bytes as .word): never pointers. Its bytes once
    # read as one function's address, a false SLOW CALL (Radio::param, Nov 16, 2026).
    table = False
    for l in dis.splitlines():
        m = re.match(r"^([0-9a-f]+) <(.*)>:", l)
        if m:
            cur = m.group(2)
            start[cur] = int(m.group(1), 16)
            table = False
            continue
        if cur is None:
            continue
        if re.search(r"\stb[bh](?:\.w)?\s", l):
            table = True
            continue
        if table:
            if re.match(r"^\s*[0-9a-f]+:\s+\.(?:word|short|byte)\b", l):
                continue
            table = False
        m = re.search(r"\s(?:b\w*|cbn?z)(?:\.w|\.n)?\s+(?:\S+, )?[0-9a-f]+ <([^>+]*)>", l)
        if m and m.group(1) != cur:
            edges[cur].add(m.group(1))
        m = re.match(r"^\s*([0-9a-f]+):\s+\.word\s+0x([0-9a-f]+)", l)
        if m:
            lits[cur].add(int(m.group(2), 16))
            pool[int(m.group(1), 16)] = int(m.group(2), 16)
        m = re.match(r"^\s*([0-9a-f]+):\s+ldr\S*\s+\w+, \[pc.*@ \(?([0-9a-f]+)", l)
        if m:
            pc_loads.append((cur, int(m.group(1), 16), int(m.group(2), 16)))
    by_addr = {a: f for f, a in start.items()}
    loads = collections.defaultdict(set)  # function -> {(value, instruction address)}
    for f, i, p in pc_loads:
        if p in pool:
            loads[f].add((pool[p], i))
    slow = {f for f, a in start.items() if lo <= a < hi}
    data = open(elf, "rb").read()
    word = lambda a, s: struct.unpack_from("<I", data, a - sec[s][0] + sec[s][1])[0]

    # Constant objects: no symbols for the unnamed ones, so an object runs from one referenced address (or symbol)
    # to the next.
    ro_secs = [s for s in (".rodata", ".slow_rodata") if s in sec]
    marks = set()
    for v in (v for ls in lits.values() for v in ls):
        if any(inside(v, s) for s in ro_secs):
            marks.add(v)
    for l in nm:
        p = l.split()
        if len(p) == 3 and any(inside(int(p[0], 16), s) for s in ro_secs):
            marks.add(int(p[0], 16))
    for s in ro_secs:
        marks.update((sec[s][0], sec[s][0] + sec[s][2]))
    marks = sorted(marks)
    numbers = []  # (start, end) of NUMBER_TABLES
    for l in run("arm-none-eabi-nm", "-S", "-C", elf).splitlines():
        p = l.split(" ", 3)
        if len(p) == 4 and NUMBER_TABLES.search(p[3]):
            numbers.append((int(p[0], 16), int(p[0], 16) + int(p[1], 16)))
    def obj(a):
        i = bisect.bisect_right(marks, a)
        return marks[i - 1], (marks[i] if i < len(marks) else a + 4)
    def is_text(a):
        a0, a1 = obj(a)
        s = next(x for x in ro_secs if inside(a0, x))
        b = data[a0 - sec[s][0] + sec[s][1]: min(a1, sec[s][0] + sec[s][2]) - sec[s][0] + sec[s][1]]
        b = b[:16].rstrip(b"\0")  # its start: strings sit back to back, and an object's end is a guess
        return all(32 <= c < 127 or c in (9, 10, 13) or c >= 0xA0 for c in b)

    # Interrupt context: handlers, then the interrupt executors' task polls.
    vt_addr, vt_off, vt_size = sec[".vector_table"]
    roots = {by_addr[struct.unpack_from("<I", data, vt_off + i)[0] & ~1] for i in range(8, vt_size, 4)
             if struct.unpack_from("<I", data, vt_off + i)[0] & ~1 in by_addr}
    polls = {f for f in start if IRQ_TASKS.search(f)}
    if len(polls) != 3:
        print(f"error: expected the 3 interrupt-executor task polls, found {len(polls)}: the task names changed?")
        return 1
    roots |= polls
    seen, stack, reads = set(roots), list(roots), collections.defaultdict(set)
    parent = {}
    fast_ro_seen = set()
    def ref(f, v, via, insn=None):
        """f (in interrupt context) loads v: a function pointer, a constant, or a table of either."""
        if (v & ~1) in by_addr and v & 1:
            g = by_addr[v & ~1]
            if g not in seen:
                parent[g] = f
                seen.add(g)
                stack.append(g)
        elif inside(v, ".slow_rodata"):
            reads[f].add((obj(v)[0], via, insn))
        elif inside(v, ".rodata"):
            if any(n0 <= v < n1 for n0, n1 in numbers):
                return  # samples, not pointers
            a0, a1 = obj(v)
            if (f, a0) in fast_ro_seen:
                return
            fast_ro_seen.add((f, a0))
            for a in range((a0 + 3) & ~3, min(a1, a0 + 4096) - 3, 4):
                w = word(a, ".rodata")
                if inside(w, ".slow_rodata") or ((w & ~1) in by_addr and w & 1):
                    ref(f, w, f"table 0x{a0:08x}")
    while stack:
        f = stack.pop()
        if f in slow:
            continue  # what a slow function reads or calls is its own business: it is already allowed below
        for g in edges.get(f, ()):
            if g in start and g not in seen:
                parent[g] = f
                seen.add(g)
                stack.append(g)
        loaded = set()
        for v, i in loads.get(f, ()):
            ref(f, v, "direct", i)
            loaded.add(v)
        for v in lits.get(f, set()) - loaded:
            ref(f, v, "direct")

    status = 0
    bad_calls = sorted(f for f in seen & slow if not any(p in d(f) for p in SLOW_CALLS))
    ok_calls = sorted(f for f in seen & slow if any(p in d(f) for p in SLOW_CALLS))
    print(f"interrupt context: {len(roots)} roots ({len(polls)} task polls), {len(seen)} functions; "
          f"slow functions reached: {len(ok_calls)} allowed, {len(bad_calls)} not")
    for f in ok_calls:
        print("    allowed:", d(f)[:100], "--", next(r for p, r in SLOW_CALLS.items() if p in d(f)))
    for f in bad_calls:
        print("    SLOW CALL:", d(f)[:140])
        g = f
        while g in parent:
            g = parent[g]
            print("        from", d(g)[:130])
        status = 1
    text_n = text_b = 0
    tables = []
    for f, rs in sorted(reads.items()):
        for a, via, insn in sorted(rs, key=lambda r: (r[0], r[1], r[2] or 0)):
            if is_text(a):
                text_n += 1
                text_b += obj(a)[1] - a
                if verbose:
                    print(f"    text  0x{a:08x} {obj(a)[1] - a:5} B  {d(f)[:90]} ({via})")
            else:
                tables.append((f, a, via, insn))
    # The source of each table read: the inlined functions at the instruction that loads it.
    insns = sorted({i for *_, i in tables if i is not None})
    frames = {}
    if insns:
        out = run("arm-none-eabi-addr2line", "-e", elf, "-i", "-f", "-C", "-a", *[hex(i) for i in insns]).splitlines()
        cur = None
        for l in out:
            if re.match(r"^0x[0-9a-f]+$", l):
                cur = int(l, 16)
                frames[cur] = []
            elif cur is not None:
                frames[cur].append(l)
    print(f"slow constants read from interrupt context: {text_n} texts ({text_b} B, fine: log and error text), "
          f"{len(tables)} tables")
    status_tables = 0
    for f, a, via, insn in tables:
        fr = frames.get(insn, [])
        names = [d(f)] + fr[0::2]
        src = " <- ".join(x.rsplit("/", 2)[-1] for x in fr[1::2][:3])
        size = obj(a)[1] - a
        why = next((r for p, z, r in SLOW_TABLES if (z is None or z[0] <= size <= z[1]) and any(re.search(p, x) for x in names)), None)
        if why:
            if verbose:
                print(f"    allowed table 0x{a:08x} {size:5} B  {names[-1][:70]} ({src}) -- {why}")
        else:
            print(f"    SLOW TABLE 0x{a:08x} {size:5} B read by {d(f)[:100]} ({via}; {src or '?'}): make it a named static")
            status_tables = 1
    status |= status_tables

    # Fast functions that can call into .slow_text at all (through direct calls).
    reach, changed = set(slow), True
    while changed:
        changed = False
        for f, es in edges.items():
            if f not in reach and es & reach:
                reach.add(f)
                changed = True
    callers = sorted(d(f) for f in reach - slow if not f.startswith("OUTLINED_FUNCTION"))
    print("fast functions that can call into .slow_text:", "none" if not callers else "")
    for f in callers:
        print("   ", f[:140])

    syms = sorted((int(p[0], 16), p[3]) for p in (l.split() for l in run("arm-none-eabi-nm", "-S", "--defined-only", elf).splitlines()) if len(p) == 4)
    addrs = [a for a, _ in syms]
    owner = lambda a: d(syms[bisect.bisect_right(addrs, a) - 1][1])
    ptrs = set()
    for name in (".rodata", ".data", ".vector_table"):
        a, off, size = sec[name]
        for i in range(0, size - 3, 4):
            v = struct.unpack_from("<I", data, off + i)[0]
            if any(n0 <= a + i < n1 for n0, n1 in numbers):
                continue  # samples, not pointers
            if v & 1 and lo <= v & ~1 < hi:
                ptrs.add(f"{name} 0x{a + i:08x} (after {owner(a + i)[:60]}) -> {owner(v & ~1)[:80]}")
    for f, vs in lits.items():
        if f not in slow:
            for v in vs:
                if v & 1 and lo <= (v & ~1) < hi:
                    ptrs.add(f"literal in {d(f)[:80]} -> {owner(v & ~1)[:80]}")
    print("fast data holding pointers into .slow_text:", "none" if not ptrs else "")
    for p in sorted(ptrs):
        print("   ", p)
    print("slowcheck:", "FAILED" if status else "ok")
    return status

if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "-v"]
    sys.exit(main(args[0] if args else "target/thumbv7em-none-eabihf/release/ferrite", "-v" in sys.argv[1:]))
