"""Measure a transmission on an RTL-SDR: carrier offset, FM deviation and the audio tones (bench checks, TX-*,
APRS-*, TERM-33). Captures `seconds` of IQ 250 kHz off the frequency (away from the dongle's DC spike), finds the keyed
part by power, FM-demodulates it and prints what it measured; `--tones` lists the strongest audio tones with their
deviation.

    sdrtx.py 145.495 --seconds 15 [--gain 0] [--ppm 0] [--save cap.u8] [--load cap.u8]
    sdrtx.py 145.495 --eye    [--save cap.u8]    # `:txtest 4fsk` on the radio, then PTT for 10 s or more
    sdrtx.py 145.495 --sweep  [--save cap.u8]    # `:txtest sweep`: hold PTT for one pass at least (8.6 s)
    sdrtx.py 145.495 --afsk   [--save cap.u8]    # `:txtest afsk` or `:txtest afsk-bk`
    sdrtx.py 0 --load cap.u8 --eye               # analyse a saved capture again (the frequency is only a label)
    sdrtx.py 0 --selftest                        # the analyses on synthetic FM IQ, no radio or SDR

The three test analyses (TERM-33, the firmware's `txtest`; signals in ferrite-core's `txtest.rs`):
- `--eye` (4FSK, M17's levels and RRC): finds the 40 ms +3/-3 preamble, filters with the matched RRC, recovers the
  symbol clock (squared-signal timing, tracked over the burst so the dongle's clock error doesn't matter), and reports
  the four levels' means and spreads (Hz), the outer/inner ratio (3.0 ideal), each eye's opening, symbol errors
  against PRBS9, and the DC drift and level droop over the run; saves an eye diagram PNG.
- `--sweep`: the 1 kHz marker, then each third-octave tone (50 Hz to 8 kHz): deviation in Hz and dB against the 1 kHz
  tone, the -3 dB points, and a response plot PNG. The audio low-pass after the demodulator is 12 kHz for it.
- `--afsk`: each tone's deviation (from the samples where the audio is at that tone) and the twist; then the tone
  switches: bit timing jitter against an ideal 1200 Bd grid (rms and peak, us, with the fitted baud), the phase step
  at each switch (typical and spread, per direction) and the discriminator's spikes above 2.6 kHz at the switches
  against mid-bit. The same for `:txtest afsk` (DAC1) and `:txtest afsk-bk` (the BK4829's tone generator).
PNGs go to --out (default: next to --load's file, else the current directory); the path is printed.

Needs librtlsdr (`rtl_sdr` on the PATH) and numpy (the project's .venv); matplotlib for the PNGs. The dongle's own
crystal error (often +-30 ppm, about +-4 kHz at 145 MHz) is in the offset unless --ppm corrects it; the deviation and
tones don't depend on it.
"""
import argparse, os, subprocess, sys, tempfile
import numpy as np

FS = 1_024_000
OFF = 250_000
DEC = 16  # 64 kS/s after the low-pass

# TERM-33's signals, as ferrite-core's txtest.rs makes them.
SYMBOL_HZ = 4800
ALPHA = 0.5
PREAMBLE_S = 0.040
SWEEP_HZ = [50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000]
MARKER_S, TONE_S, GAP_S = 0.5, 0.3, 0.05


def lowpass(cut, fs, taps=255):
    n = np.arange(taps) - (taps - 1) / 2
    h = np.sinc(2 * cut / fs * n) * np.hamming(taps)
    return h / h.sum()


def conv_same(x, h):
    """np.convolve(x, h, "same") by FFT: the same numbers, fast for long captures."""
    n = len(x) + len(h) - 1
    m = 1 << (n - 1).bit_length()
    y = np.fft.ifft(np.fft.fft(x, m) * np.fft.fft(h, m))[:n] if np.iscomplexobj(x) else np.fft.irfft(np.fft.rfft(x, m) * np.fft.rfft(h, m), m)[:n]
    s = (len(h) - 1) // 2
    return y[s : s + len(x)]


def rrc(t, alpha=ALPHA):
    """The root-raised-cosine at t symbols (unit area pulse), with its two singular points."""
    t = np.asarray(t, float)
    out = np.empty_like(t)
    z = np.isclose(t, 0)
    s = np.isclose(np.abs(4 * alpha * t), 1)
    r = ~(z | s)
    out[z] = 1 - alpha + 4 * alpha / np.pi
    a = np.pi / (4 * alpha)
    out[s] = alpha / np.sqrt(2) * ((1 + 2 / np.pi) * np.sin(a) + (1 - 2 / np.pi) * np.cos(a))
    tr = t[r]
    out[r] = (np.sin(np.pi * tr * (1 - alpha)) + 4 * alpha * tr * np.cos(np.pi * tr * (1 + alpha))) / (np.pi * tr * (1 - (4 * alpha * tr) ** 2))
    return out


def prbs9(n):
    """PRBS9 (x^9 + x^5 + 1), as ferrite-core's Prbs9 from 0x1FF."""
    s, out = 0x1FF, np.empty(n, np.uint8)
    for i in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out[i] = b
    return out


def m17_levels(bits):
    """M17's dibits to symbols: 01 +3, 00 +1, 10 -1, 11 -3."""
    hi, lo = bits[0::2], bits[1::2]
    return np.where(hi == 0, np.where(lo == 1, 3, 1), np.where(lo == 0, -1, -3)).astype(int)


def prbs_symbols():
    """One period of the test's PRBS symbols (511: two of the PRBS's periods)."""
    return m17_levels(prbs9(1022))


# ---- Capture and demodulation ----


def capture(o):
    f0 = o.mhz * 1e6
    n = int(o.seconds * FS) * 2
    cmd = ["rtl_sdr", "-f", str(int(f0 + OFF)), "-s", str(FS), "-g", str(o.gain), "-p", str(o.ppm), "-n", str(n // 2), "-"]
    print("capturing", o.seconds, "s at", o.mhz, "MHz (gain", o.gain, "dB): key up now", file=sys.stderr)
    raw = np.frombuffer(subprocess.run(cmd, capture_output=True).stdout, np.uint8)
    if o.save:
        raw.tofile(o.save)
    return raw


def baseband(raw, chunk=1 << 20):
    """rtl_sdr's u8 IQ to complex baseband at FS / DEC: the signal moved from -OFF to 0 Hz, low-passed at 20 kHz and
    decimated; the same numbers as conv_same over the whole capture, a chunk at a time (overlap-save) so a minute's
    capture fits in memory."""
    h = lowpass(20_000, FS)
    L = len(h)
    s = (L - 1) // 2
    n = len(raw) // 2
    out = []
    m = 1 << (chunk + L - 2).bit_length()
    H = np.fft.fft(h, m)
    for a in range(0, n, chunk):
        b = min(a + chunk, n)
        lo, hi = a - s, b + s  # the inputs y[a:b] need
        x = np.zeros(hi - lo, np.complex128)
        i0, i1 = max(lo, 0), min(hi, n)
        seg = raw[2 * i0 : 2 * i1].astype(np.float64) - 127.5
        k = np.arange(i0, i1)
        x[i0 - lo : i1 - lo] = (seg[0::2] + 1j * seg[1::2]) * np.exp(2j * np.pi * OFF * (k % FS) / FS)
        y = np.fft.ifft(np.fft.fft(x, m) * H)[L - 1 : len(x)]
        out.append(y[::DEC].astype(np.complex64))
    return np.concatenate(out) if out else np.zeros(0, np.complex64)


def demod(raw, mhz, audio_lp, quiet=False):
    """The keyed part's FM deviation (Hz, carrier offset removed) at 64 kS/s, and the offset; None if no
    transmission."""
    clip = np.mean((raw == 0) | (raw == 255))
    x = baseband(raw)
    fs = FS / DEC
    p = np.abs(x) ** 2
    blk = int(fs * 0.01)
    pb = p[: len(p) // blk * blk].reshape(-1, blk).mean(1)
    floor = np.percentile(pb, 10)  # the quietest tenth: a capture can be mostly keyed
    if floor > 1e3:  # keyed through nearly all of it: an unkeyed dongle reads well under 30 dB here
        floor = 1.0
    on = pb > floor * 100  # 20 dB over the floor
    if not on.any():
        print(f"no transmission found (floor {10*np.log10(floor+1e-12):.1f} dB, clipping {clip:.2%})")
        return None
    i0, i1 = np.argmax(on), len(on) - np.argmax(on[::-1])
    seg = x[(i0 + 2) * blk : (i1 - 2) * blk]  # 20 ms in from each end: no key-up transients
    fm = np.angle(seg[1:] * np.conj(seg[:-1])) * fs / (2 * np.pi)
    fm = conv_same(fm, lowpass(audio_lp, fs))
    off = np.mean(fm)
    dev = fm - off
    if not quiet:
        print(f"keyed {(i1 - i0) * 0.01:.2f} s, {10*np.log10(pb[i0:i1].mean()/floor):.0f} dB over the floor, IQ clipping {clip:.2%}")
        print(f"carrier offset {off:+.0f} Hz ({off / max(mhz, 1e-9):+.1f} ppm, the dongle's error included unless --ppm)" if mhz else f"carrier offset {off:+.0f} Hz")
        print(f"deviation: peak {np.max(np.abs(dev)):.0f} Hz, 99.9 % {np.percentile(np.abs(dev), 99.9):.0f} Hz, rms {np.sqrt(np.mean(dev**2)):.0f} Hz (audio low-pass {audio_lp/1000:g} kHz)")
    return dev, fs, off


def spectrum_report(dev, fs, tones):
    spec = np.abs(np.fft.rfft(dev * np.hanning(len(dev)))) * 2 / (np.hanning(len(dev)).sum())
    fr = np.fft.rfftfreq(len(dev), 1 / fs)

    def tone(hz, bw=40):
        m = (fr > hz - bw) & (fr < hz + bw)
        k = np.argmax(spec * m)
        return fr[k], spec[k]

    f12, a12 = tone(1200)
    f22, a22 = tone(2200)
    if a12 > 0 and a22 > 0:
        print(f"AFSK lines: {f12:.0f} Hz at {a12:.0f} Hz dev, {f22:.0f} Hz at {a22:.0f} Hz dev, twist (2200 over 1200) {20*np.log10(a22/a12):+.1f} dB")
    if tones:
        band = (fr > 50) & (fr < 6000)
        top = np.argsort(spec * band)[::-1]
        seen = []
        for k in top:
            if all(abs(fr[k] - s) > 30 for s in seen):
                seen.append(fr[k])
                print(f"  tone {fr[k]:7.1f} Hz  {spec[k]:6.0f} Hz dev")
            if len(seen) == 8:
                break


def pyplot():
    try:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt

        return plt
    except ImportError:
        print("  (no PNG: matplotlib isn't installed; .venv/bin/pip install matplotlib)")
        return None


# ---- 4FSK: --eye ----


def tone_env(x, fs, hz, ms):
    """The amplitude of the `hz` component of x, smoothed over `ms`."""
    z = x * np.exp(-2j * np.pi * hz * np.arange(len(x)) / fs)
    n = max(1, int(fs * ms / 1000))
    return 2 * np.abs(np.convolve(z, np.ones(n) / n, "same"))


def kmeans4(v):
    c = np.percentile(v, [12.5, 37.5, 62.5, 87.5])
    for _ in range(30):
        lab = np.argmin(np.abs(v[:, None] - c[None, :]), 1)
        c = np.array([v[lab == i].mean() if np.any(lab == i) else c[i] for i in range(4)])
    return c, np.argmin(np.abs(v[:, None] - c[None, :]), 1)


EYE_SKIP_S = 0.0  # --skip: start the 4FSK analysis this long after the preamble


def eye(dev, fs, out_png=None):
    """4FSK analysis; returns the figures it printed (for --selftest)."""
    sps = fs / SYMBOL_HZ
    r = {}
    # The preamble: +3/-3 alternation, a 2400 Hz tone, at the start of the test signal.
    env = tone_env(dev, fs, SYMBOL_HZ / 2, 5)
    first = np.argmax(env > 0.5 * np.max(env[: int(fs * 2)])) if len(env) > fs * 0.1 else 0
    data0 = first + int(fs * (PREAMBLE_S + 2 / SYMBOL_HZ)) + int(fs * EYE_SKIP_S)
    print(f"4FSK: preamble found {first / fs * 1000:.0f} ms into the keyed part; data from {data0 / fs * 1000:.0f} ms")
    # The matched filter: RRC, 8 symbols, unit sum (a symbol centre then reads its level in Hz).
    span = int(np.ceil(4 * sps))
    h = rrc(np.arange(-span, span + 1) / sps)
    h /= h.sum()
    y = conv_same(dev, h)
    y = y[: len(y) - span]
    if len(y) - data0 < fs * 0.5:
        print("  too short: hold PTT for a few seconds at least")
        return None
    # Symbol timing: the squared signal's symbol-rate component per 0.2 s block gives the centres' phase; its
    # drift over the blocks is the clocks' difference (the radio's and the dongle's), fitted as a line.
    blk = int(fs * 0.2)
    cs, ts = [], []
    for b in range(data0, len(y) - blk, blk):
        n = np.arange(b, b + blk)
        c = np.sum(y[n] ** 2 * np.exp(-2j * np.pi * n / sps))
        cs.append(-np.angle(c) / (2 * np.pi))  # centre phase, in symbols
        ts.append(b + blk / 2)
    ph = np.unwrap(np.array(cs) * 2 * np.pi) / (2 * np.pi)
    ts = np.array(ts)
    if len(ts) >= 2:
        slope, icpt = np.polyfit(ts, ph, 1)
    else:
        slope, icpt = 0.0, ph[0]
    # Centres at n where n / sps - (icpt + slope n) is an integer k.
    k0 = int(np.ceil(data0 / sps - (icpt + slope * data0)))
    k1 = int(np.floor(len(y) / sps - (icpt + slope * len(y)))) - 1
    k = np.arange(k0, k1)
    nk0 = (k + icpt) / (1 / sps - slope)

    # The squared signal's phase finds the clock but not reliably the centre (it can land on the transitions,
    # Oct 13, 2026 on the radio): of 32 offsets across a symbol, keep the one with the most open eye.
    def at(off):
        n = nk0 + off * sps
        n = n[(n > data0) & (n < len(y) - 2)]
        return n, np.interp(n, np.arange(len(y)), y)

    def openness(v):
        cc, ll = kmeans4(v)
        cc = np.sort(cc)
        sp = np.mean([v[ll == i].std() for i in range(4) if np.any(ll == i)])
        return np.min(np.diff(cc)) / max(sp, 1e-9)

    off = max(np.arange(32) / 32 - 0.5, key=lambda o: openness(at(o)[1][:20000]))
    nk, v = at(off)
    if abs(off) > 1e-9:
        print(f"  centre found {off:+.2f} symbol from the clock estimate")
    ppm = slope * sps * 1e6
    print(f"  {len(v)} symbols; symbol clock {SYMBOL_HZ * (1 + slope * sps):.2f} Bd against the dongle's ({ppm:+.0f} ppm, both clocks' error)")
    c, lab = kmeans4(v)
    order = np.argsort(c)
    c = c[order]
    lab = np.argsort(order)[lab]
    sd = np.array([v[lab == i].std() for i in range(4)])
    names = ["-3", "-1", "+1", "+3"]
    print("  levels (Hz):  " + "   ".join(f"{names[i]} {c[i]:+7.0f} +-{sd[i]:4.0f}" for i in range(4)) + "   (M17: -2400 -800 +800 +2400)")
    outer, inner = c[3] - c[0], c[2] - c[1]
    ratio = outer / inner
    print(f"  outer/inner: {ratio:.3f} (span {outer:.0f} / {inner:.0f} Hz; ideal 3.000); +3/+1 {c[3]/c[2]:.3f}, -3/-1 {c[0]/c[1]:.3f}")
    eyes = []
    for i in range(3):
        lo, hi = v[lab == i], v[lab == i + 1]
        op = (np.percentile(hi, 0.1) - np.percentile(lo, 99.9)) / (c[i + 1] - c[i]) * 100
        eyes.append(op)
    print("  eye opening (0.1 % / 99.9 % of each level, of the spacing): " + ", ".join(f"{names[i]}/{names[i+1]} {eyes[i]:.0f} %" for i in range(3)))
    # Symbol errors: the decided symbols against PRBS9's 511, aligned on the first 1000.
    dec = np.array([-3, -1, 1, 3])[lab]
    ref = prbs_symbols()
    m = min(1000, len(dec))
    # Either polarity: the chain from the DAC to the deviation inverts (measured Oct 13, 2026), which Ferrite's
    # generator now undoes (TERM-33), so "polarity inverted" here means a build before that or a flipped dongle.
    def align(d):
        best = max(range(511), key=lambda s: np.sum(d[:m] == np.roll(ref, -s)[np.arange(m) % 511]))
        return int(np.sum(d != np.roll(ref, -best)[np.arange(len(d)) % 511]))

    errs, inv = min((align(dec), False), (align(-dec), True))
    # Which level carries which symbol, if not M17's order: the best of the 24 maps on the first 2000 symbols.
    import itertools
    best_map = min(itertools.permutations([-3, -1, 1, 3]), key=lambda pm: align(np.array(pm)[lab[:2000]]))
    if list(best_map) != [-3, -1, 1, 3]:
        alt = align(np.array(best_map)[lab])
        print(f"  levels low to high carry {list(best_map)} (M17's order is [-3, -1, 1, 3]): {alt} errors that way ({alt / len(dec):.2e})")
    # Slips (the DAC dropping or repeating a block) show as errors that jump after a clean start: per 2000
    # symbols, each aligned on its own.
    d = -dec if inv else dec
    parts = [align(d[i : i + 2000]) / 2000 for i in range(0, len(d) - 2000, 2000)]
    if parts:
        print("  errors per 2000 symbols, each aligned alone: " + " ".join(f"{e:.0%}" for e in parts))
    print(f"  symbol errors against PRBS9: {errs} of {len(dec)} ({errs / len(dec):.2e}){', polarity inverted' if inv else ''}")
    # Drift and droop: per 0.5 s, the mean of all symbols (DC) and the outer span.
    per = int(SYMBOL_HZ * 0.5)
    dcs, spans = [], []
    for b in range(0, len(v) - per + 1, per):
        vv, ll = v[b : b + per], lab[b : b + per]
        dcs.append(vv.mean())
        if np.any(ll == 3) and np.any(ll == 0):
            spans.append(vv[ll == 3].mean() - vv[ll == 0].mean())
    dcs = np.array(dcs)
    if len(dcs) >= 2:
        dslope = np.polyfit(np.arange(len(dcs)) * 0.5, dcs, 1)[0]
        print(f"  DC per 0.5 s: {dcs.min():+.0f} to {dcs.max():+.0f} Hz (range {np.ptp(dcs):.0f} Hz, trend {dslope:+.1f} Hz/s)")
    if len(spans) >= 2:
        print(f"  outer span per 0.5 s: first {spans[0]:.0f} Hz, last {spans[-1]:.0f} Hz ({(spans[-1] / spans[0] - 1) * 100:+.1f} %), min {min(spans):.0f}, max {max(spans):.0f}")
    r.update(levels=c, spread=sd, ratio=ratio, eyes=eyes, errors=errs, symbols=len(dec), dc_range=float(np.ptp(dcs)) if len(dcs) else 0.0)
    plt = pyplot() if out_png else None
    if plt:
        fig, ax = plt.subplots(figsize=(8, 5))
        tt = np.linspace(-1, 1, 81)
        idx = np.linspace(0, len(nk) - 1, min(3000, len(nk))).astype(int)
        for n in nk[idx]:
            pos = n + tt * sps
            if pos[0] >= 0 and pos[-1] < len(y):
                ax.plot(tt, np.interp(pos, np.arange(len(y)), y), color="#2a6fdb", alpha=0.04, lw=0.8)
        for lv in c:
            ax.axhline(lv, color="#c0392b", lw=0.8, ls="--")
        ax.set_xlabel("symbols from the centre")
        ax.set_ylabel("deviation (Hz), after the matched RRC")
        ax.set_title(f"4FSK eye: levels {', '.join(f'{x:+.0f}' for x in c)} Hz, outer/inner {ratio:.2f}")
        ax.grid(alpha=0.3)
        fig.tight_layout()
        fig.savefig(out_png, dpi=120)
        plt.close(fig)
        print(f"  eye diagram: {out_png}")
    return r


# ---- --sweep ----


def fit_amp(x, fs, hz):
    """Least-squares amplitude of a sine at `hz` in x."""
    n = np.arange(len(x))
    a = np.column_stack([np.sin(2 * np.pi * hz * n / fs), np.cos(2 * np.pi * hz * n / fs), np.ones(len(x))])
    s, c, _ = np.linalg.lstsq(a, x, rcond=None)[0]
    return float(np.hypot(s, c))


def sweep(dev, fs, out_png=None):
    # The 1 kHz marker: 500 ms where the sweep's own 1 kHz tone is 300 ms.
    env = tone_env(dev, fs, 1000, 10)
    tot = np.sqrt(2 * np.convolve(dev**2, np.ones(int(fs * 0.01)) / int(fs * 0.01), "same"))
    is1k = (env > 0.7 * tot) & (env > 0.2 * np.max(env))
    starts, i, n = [], 0, len(is1k)
    while i < n:
        if is1k[i]:
            j = i
            while j < n and is1k[j]:
                j += 1
            if (j - i) / fs > 0.42:
                starts.append(i)
            i = j
        else:
            i += 1
    period = MARKER_S + GAP_S + len(SWEEP_HZ) * (TONE_S + GAP_S)
    print(f"sweep: {len(starts)} marker(s) found; a pass is {period:.2f} s")
    if not starts:
        print("  no 1 kHz marker: was `:txtest sweep` armed, and PTT held for a pass?")
        return None
    # The pass before the first whole marker too, when its tones are in the capture: a capture started after the
    # key-up (or trimmed at it) cuts the first marker short, and a key-up of about a pass then held only the tones
    # after the second marker's start (TERM-33.16 bench, Oct 27).
    before = starts[0] - int(fs * period)
    if before + int(fs * (MARKER_S + GAP_S)) >= 0:
        starts.insert(0, before)
        print("  and the pass before the first marker (its marker cut short)")
    firsts = [s + int(fs * (MARKER_S + GAP_S)) for s in starts]
    fr, a = sweep_measure(dev, fs, firsts, TONE_S, GAP_S)
    return sweep_report(fr, a, out_png)


def sweep_measure(dev, fs, firsts, tone_s, gap_s):
    """Each sweep tone's deviation (Hz, the median over the passes) for passes whose first tone starts at the samples
    `firsts`, each tone `tone_s` long with `gap_s` after: a sine fitted to its middle (50 ms clear of each end)."""
    amps = {f: [] for f in SWEEP_HZ}
    for s in firsts:
        for i, f in enumerate(SWEEP_HZ):
            a = s + int(fs * (i * (tone_s + gap_s) + 0.05))
            b = a + int(fs * (tone_s - 0.1))
            if a >= 0 and b <= len(dev):
                # The phase-difference discriminator's own response, sin(pi f/fs) / (pi f/fs): -0.2 dB at 8 kHz.
                amps[f].append(fit_amp(dev[a:b], fs, f) / np.sinc(f / fs))
    fr = [f for f in SWEEP_HZ if amps[f]]
    return fr, np.array([np.median(amps[f]) for f in fr])


def minus3(fr, db):
    """The -3 dB points against 1 kHz (log-interpolated), low and high; None where the response doesn't fall that
    far."""
    i1 = fr.index(1000)

    def cross(rng):
        prev = i1
        for i in rng:
            if db[i] < -3:
                lf = np.interp(-3, [db[i], db[prev]], [np.log10(fr[i]), np.log10(fr[prev])])
                return 10**lf
            prev = i
        return None

    return cross(range(i1 - 1, -1, -1)), cross(range(i1 + 1, len(fr)))


def sweep_report(fr, a, out_png=None):
    """The response table against 1 kHz and the -3 dB points (printed), the plot PNG if asked."""
    if 1000 not in fr:
        print("  the pass is cut short before 1 kHz")
        return None
    ref = a[fr.index(1000)]
    db = 20 * np.log10(np.maximum(a, 1e-3) / ref)
    print("    Hz     dev Hz    dB (1 kHz = 0)")
    for f, x, d in zip(fr, a, db):
        print(f"  {f:5d}  {x:8.0f}  {d:+7.1f}")
    lo, hi = minus3(fr, db)
    print(f"  -3 dB points: low {f'{lo:.0f} Hz' if lo else f'below {fr[0]} Hz'}, high {f'{hi:.0f} Hz' if hi else f'above {fr[-1]} Hz'}")
    plt = pyplot() if out_png else None
    if plt:
        fig, ax = plt.subplots(figsize=(8, 5))
        ax.semilogx(fr, db, "o-", color="#2a6fdb")
        ax.axhline(-3, color="#c0392b", lw=0.8, ls="--")
        for p in (lo, hi):
            if p:
                ax.axvline(p, color="#c0392b", lw=0.8, ls=":")
        ticks = [50, 100, 200, 300, 500, 1000, 2000, 3150, 5000, 8000]
        ax.set_xticks(ticks, ["50", "100", "200", "300", "500", "1k", "2k", "3.15k", "5k", "8k"])
        ax.set_xticks([], minor=True)
        ax.set_xlabel("audio frequency (Hz)")
        ax.set_ylabel("deviation, dB against 1 kHz")
        ax.set_title(f"TX response (1 kHz = {ref:.0f} Hz dev); -3 dB {lo or 0:.0f} Hz to {hi or 0:.0f} Hz")
        ax.grid(which="both", alpha=0.3)
        fig.tight_layout()
        fig.savefig(out_png, dpi=120)
        plt.close(fig)
        print(f"  response plot: {out_png}")
    return dict(freqs=fr, dev=a, db=db, low=lo, high=hi, ref=ref)


# ---- --afsk ----


def afsk(dev, fs):
    """Each tone's deviation and the twist: every bit fitted with both tones (least squares over its middle, the
    edges left out), the bit clock found per 0.25 s block as the phase where the bits come out purest (one tone
    each). Flags are seven bits of one tone and one of the other; each tone's figure is the median over its bits."""
    per = fs / 1200
    L = int(per) - 8
    t = np.arange(L) / fs
    B = np.column_stack([np.sin(2 * np.pi * 1200 * t), np.cos(2 * np.pi * 1200 * t), np.sin(2 * np.pi * 2200 * t), np.cos(2 * np.pi * 2200 * t), np.ones(L)])
    P = np.linalg.pinv(B)
    marks, spaces = [], []
    blk = int(0.25 * 1200)
    nbits = int(len(dev) / per) - 2
    for b0 in range(0, nbits - blk, blk):
        best = None
        for ph in np.arange(0, per, 2):
            st = np.round(ph + (b0 + np.arange(blk)) * per + 4).astype(int)
            X = dev[st[:, None] + np.arange(L)[None, :]]
            c = X @ P.T
            am, asp = np.hypot(c[:, 0], c[:, 1]), np.hypot(c[:, 2], c[:, 3])
            pur = np.mean(np.maximum(am, asp) / (am + asp + 1e-9))
            if best is None or pur > best[0]:
                best = (pur, am, asp)
        _, am, asp = best
        m = am > asp
        marks += list(am[m])
        spaces += list(asp[~m])
    res = {1200: float(np.median(marks)) if marks else 0.0, 2200: float(np.median(spaces)) if spaces else 0.0}
    print(f"  mark 1200: {res[1200]:.0f} Hz deviation ({len(marks)} bits); space 2200: {res[2200]:.0f} Hz ({len(spaces)} bits)")
    if res[1200] > 0 and res[2200] > 0:
        tw = 20 * np.log10(res[2200] / res[1200])
        print(f"  twist (2200 over 1200): {tw:+.2f} dB")
        res["twist"] = tw
        res.update(afsk_edges(dev, fs, res[1200], res[2200]))
    return res


def tone_switches(dev, fs, a_mark, a_space):
    """The tone switch instants (s), each with its direction (True: to mark): where the two tones' amplitudes, each
    over its own median and smoothed by a 600 Hz low-pass (symmetric, so no delay), cross."""
    t = np.arange(len(dev)) / fs
    h = lowpass(600, fs, 2 * int(fs / 1200) + 1)
    am = np.abs(conv_same(dev * np.exp(-2j * np.pi * 1200 * t), h)) / a_mark
    asp = np.abs(conv_same(dev * np.exp(-2j * np.pi * 2200 * t), h)) / a_space
    d = am - asp
    # Hysteresis: a switch counts once the difference has gone a tenth of the way to the other tone; its instant
    # is the last zero crossing before that (a phase jump's spike may cross and cross back first).
    h = 0.2
    side = np.where(d > h, 1, np.where(d < -h, -1, 0))
    nz = np.nonzero(side)[0]
    if len(nz) < 2:
        return np.array([]), np.array([], bool)
    flips = nz[1:][side[nz[1:]] != side[nz[:-1]]]
    cross = np.nonzero(np.sign(d[1:]) != np.sign(d[:-1]))[0]
    out_t, out_up = [], []
    for j in flips:
        k = np.searchsorted(cross, j) - 1
        if k < 0:
            continue
        i = cross[k]
        out_t.append((i + d[i] / (d[i] - d[i + 1])) / fs)
        out_up.append(side[j] > 0)
    return np.array(out_t), np.array(out_up)


def afsk_edges(dev, fs, a_mark, a_space):
    """Bit timing and phase continuity at the tone switches (TERM-33, for `afsk` from DAC1 and `afsk-bk` from the
    BK4829's generator alike).

    Timing: the bit period that lines the switch instants up best (the radio's baud with the dongle's clock error in
    it), then each switch's place within its bit against the mean place of its kind (the gap before it, 1 bit or
    more, and its direction: the detector's own bias at a 1-bit run, the same every time); the spread is the
    jitter: rms, peak and 99 %, microseconds. Phase jumps dent the tones' amplitudes and move the detected instants
    too, so a large phase spread inflates the jitter: read the two together.

    Phase: each switch's two tones are fitted on either side (0.4 bit each, 0.1 bit clear of the switch) and carried
    to the switch: the phase step there (degrees). A phase-continuous source gives a constant step per direction
    (the receive path's delay differs per tone), so its spread is what counts. Spikes: the discriminator's output
    above 2.6 kHz (both tones and their sidebands under it), its peak within 0.2 bit of each switch against the same
    at mid-bit in the 7-bit runs; a phase jump shows as a spike at the switch."""
    T0 = 1 / 1200
    ts, up = tone_switches(dev, fs, a_mark, a_space)
    out = {}
    if len(ts) < 20:  # also none at all: no tones, or the capture's IQ mirrored (swap I and Q for the bladeRF)
        print("  bit timing: too few tone switches found")
        return out
    # Keep the switches at least half a bit after the last kept (a noise crossing inside a bit falls out) and
    # number them by their gaps rounded to whole bits.
    keep = [0]
    for k in range(1, len(ts)):
        if (ts[k] - ts[keep[-1]]) / T0 > 0.5:
            keep.append(k)
    ts, up = ts[keep], up[keep]
    if len(ts) < 20:
        print("  bit timing: too few tone switches found")
        return out
    gaps = np.round(np.diff(ts) / T0).astype(int)
    use = np.arange(len(ts)) > 0
    kind = np.concatenate([[0], np.minimum(gaps, 2)]) * 2 + up  # the gap before (1 bit, or more) and the direction
    # The bit clock: the period that lines the switches up best (each switch's place within its bit, as a phase,
    # most concentrated), searched over +-500 ppm (the dongle's clock error), then the place's spread per kind of
    # switch. Counting no bits, a missed or an extra switch costs one point, not the fit.
    tu, ku = ts[use], kind[use]
    def conc(T):
        z = np.exp(2j * np.pi * tu / T)
        return sum(abs(z[ku == k].sum()) for k in set(ku))
    grid = T0 / (1 + np.arange(-500, 501, 2) * 1e-6)
    T = grid[np.argmax([conc(x) for x in grid])]
    fine = T * (1 + np.arange(-3, 3.01, 0.05) * 1e-6)
    T = fine[np.argmax([conc(x) for x in fine])]
    ph = np.angle(np.exp(2j * np.pi * tu / T))
    resid = np.empty_like(ph)
    bias = {}
    for k in set(ku):
        m = np.angle(np.mean(np.exp(1j * ph[ku == k])))
        resid[ku == k] = np.angle(np.exp(1j * (ph[ku == k] - m)))
        bias[k] = m * T / (2 * np.pi) * 1e6
    jit = resid * T / (2 * np.pi) * 1e6
    nbits = int(round((ts[-1] - ts[0]) / T))
    out.update(period_us=T * 1e6, baud_ppm=(T0 / T - 1) * 1e6, jitter_rms_us=float(np.sqrt(np.mean(jit**2))), jitter_peak_us=float(np.max(np.abs(jit))), switches=int(use.sum()))
    print(f"  bit timing: {out['switches']} tone switches over {nbits} bits; bit {out['period_us']:.3f} us "
          f"({out['baud_ppm']:+.0f} ppm against 1200 Bd, the dongle's clock error included)")
    print(f"  jitter of the switches against the ideal 1200 Bd grid: rms {out['jitter_rms_us']:.1f} us, peak {out['jitter_peak_us']:.1f} us, "
          f"99 % {np.percentile(np.abs(jit), 99):.1f} us ({out['jitter_rms_us'] / (T0 * 1e6) * 100:.2f} % of a bit rms)")
    ref = np.median(list(bias.values()))
    rel = {k: np.angle(np.exp(2j * np.pi * (v - ref) / (T * 1e6))) * T * 1e6 / (2 * np.pi) for k, v in bias.items()}
    print("  detector offset per switch kind (us; gap 1 = after a 1-bit tone): " + ", ".join(
        f"gap {'1' if k < 4 else '2+'} to {'mark' if k % 2 else 'space'} {v:+.1f}" for k, v in sorted(rel.items())))
    # Phase step at each switch.
    w, clear = int(0.4 * T0 * fs), int(0.1 * T0 * fs)
    def fit(seg_t, x, hz):
        B = np.column_stack([np.cos(2 * np.pi * hz * seg_t), np.sin(2 * np.pi * hz * seg_t), np.ones(len(seg_t))])
        c, *_ = np.linalg.lstsq(B, x, rcond=None)
        return np.arctan2(-c[1], c[0])  # x ~ cos(2 pi f (t - ts) + phase)
    steps = {True: [], False: []}
    for k in np.nonzero(use)[0]:
        c = int(round(ts[k] * fs))
        if c - clear - w < 0 or c + clear + w > len(dev):
            continue
        before, after = (1200, 2200) if not up[k] else (2200, 1200)
        tb = (np.arange(c - clear - w, c - clear) / fs) - ts[k]
        ta = (np.arange(c + clear, c + clear + w) / fs) - ts[k]
        pb = fit(tb, dev[c - clear - w : c - clear], before)
        pa = fit(ta, dev[c + clear : c + clear + w], after)
        steps[bool(up[k])].append(np.angle(np.exp(1j * (pa - pb))))
    for d, name in ((True, "to mark"), (False, "to space")):
        v = np.array(steps[d])
        if len(v) < 5:
            continue
        m = np.angle(np.mean(np.exp(1j * v)))
        spread = np.degrees(np.sqrt(-2 * np.log(max(np.abs(np.mean(np.exp(1j * v))), 1e-9))))
        out[f"phase_{name.split()[1]}"] = (float(np.degrees(m)), float(spread))
        print(f"  phase step at the switch {name}: {np.degrees(m):+.0f} deg typical, spread {spread:.0f} deg (circular sd; "
              f"near 0 for a phase-continuous source, tens of degrees when each switch jumps)")
    # Spikes: above 2.6 kHz, at the switches against mid-bit.
    f = np.fft.rfftfreq(len(dev), 1 / fs)
    hi = np.abs(np.fft.irfft(np.fft.rfft(dev) * (f > 2600), len(dev)))
    r2 = int(0.2 * T0 * fs)
    def peak(tc):
        c = int(round(tc * fs))
        return hi[max(c - r2, 0) : c + r2 + 1].max() if r2 < c < len(hi) - r2 else np.nan
    at_sw = np.array([peak(x) for x in ts[use]])
    mids = [ts[k - 1] + (j + 0.5) * T for k in np.nonzero(use)[0] if gaps[k - 1] >= 5 for j in range(1, gaps[k - 1] - 1)]
    at_mid = np.array([peak(x) for x in mids])
    if np.isfinite(at_sw).any() and np.isfinite(at_mid).any():
        es, em = np.nanmedian(at_sw), np.nanmedian(at_mid)
        out["spike_db"] = float(20 * np.log10(es / max(em, 1e-9)))
        out["spike_hz"] = float(es)
        print(f"  discriminator above 2.6 kHz: {es:.0f} Hz peak at the switches, {em:.0f} Hz mid-bit (median): "
              f"{out['spike_db']:+.1f} dB at the switches")
    return out


# ---- --selftest ----


def gen_fsk4(secs, rate=48_000):
    """The firmware's 4FSK at 48 kHz: preamble, then PRBS9 symbols, RRC (sum 10), +3 = 1 unit * 3."""
    sps = rate // SYMBOL_HZ
    nsym = int(secs * SYMBOL_HZ)
    pre = int(PREAMBLE_S * SYMBOL_HZ)
    ref = prbs_symbols()
    s = np.concatenate([np.tile([3, -3], pre // 2), np.resize(ref, nsym - pre)])
    up = np.zeros(nsym * sps)
    up[::sps] = s
    h = rrc(np.arange(-40, 41) / sps)
    h = h * sps / h.sum()
    return np.convolve(up, h)[40 : 40 + len(up)]


def gen_sweep(passes, rate=48_000):
    out = []
    for _ in range(passes):
        t = np.arange(int(rate * MARKER_S)) / rate
        out += [np.sin(2 * np.pi * 1000 * t), np.zeros(int(rate * GAP_S))]
        t = np.arange(int(rate * TONE_S)) / rate
        for f in SWEEP_HZ:
            out += [np.sin(2 * np.pi * f * t), np.zeros(int(rate * GAP_S))]
    return np.concatenate(out)


def gen_afsk(secs, space_gain, rate=48_000, jitter_us=0.0, phase_jump=False, rng=None):
    """Bell 202 flags, NRZI (a 0 changes the tone; the first bit on space, as the firmware), phase-continuous; the
    space tone `space_gain` times the mark's. `jitter_us`: each switch moved by a uniform random time up to that
    (as the BK4829 path's late writes); `phase_jump`: each tone starts again from a random phase at a switch."""
    n = int(secs * rate)
    nb = n * 1200 // rate + 2
    flag = (0x7E >> (np.arange(nb) % 8)) & 1
    mark_bit = (np.cumsum(flag == 0) % 2) == 0
    edges = np.arange(nb + 1) / 1200.0
    if jitter_us:
        edges = edges + rng.uniform(0, jitter_us * 1e-6, len(edges))
    t = np.arange(n) / rate
    bit = np.searchsorted(edges, t, side="right") - 1
    mark = mark_bit[bit]
    hz = np.where(mark, 1200, 2200)
    if phase_jump:
        changed = np.concatenate([[True], mark_bit[1:] != mark_bit[:-1]])
        last = np.maximum.accumulate(np.where(changed, np.arange(nb), 0))  # the bit each tone run started at
        ph = 2 * np.pi * hz * (t - edges[last[bit]]) + rng.uniform(0, 2 * np.pi, nb)[last[bit]]
    else:
        ph = 2 * np.pi * np.cumsum(hz) / rate
    return np.sin(ph) * np.where(mark, 1.0, space_gain)


def to_iq(dev_hz, rate, clock_ppm, offset_hz, rng, lead=1.0):
    """FM IQ at FS as rtl_sdr writes it (u8 interleaved), the signal at -OFF: band-limited resampling of the 48 kHz
    deviation (played `clock_ppm` fast against the dongle), silence before and after, noise."""
    n = len(dev_hz)
    m = int(round(n * FS / (rate * (1 + clock_ppm * 1e-6))))
    sp = np.fft.rfft(dev_hz)
    up = np.fft.irfft(sp, m) * m / n
    lead_n = int(lead * FS)
    f = np.concatenate([np.zeros(lead_n), up + offset_hz, np.zeros(lead_n)]) - OFF
    ph = 2 * np.pi * np.cumsum(f) / FS
    amp = np.concatenate([np.zeros(lead_n), np.full(m, 60.0), np.zeros(lead_n)])
    x = amp * np.exp(1j * ph) + rng.normal(0, 1.5, len(f)) + 1j * rng.normal(0, 1.5, len(f))
    raw = np.empty(2 * len(x), np.uint8)
    raw[0::2] = np.clip(np.round(x.real + 127.5), 0, 255)
    raw[1::2] = np.clip(np.round(x.imag + 127.5), 0, 255)
    return raw


def response(x, rate, f_hp, f_lp):
    """A first-order high-pass and low-pass (a stand-in for the radio's audio path), by FFT."""
    f = np.fft.rfftfreq(len(x), 1 / rate)
    h = (1j * f / f_hp) / (1 + 1j * f / f_hp) / (1 + 1j * f / f_lp)
    return np.fft.irfft(np.fft.rfft(x) * h, len(x))


def selftest(out_dir):
    rng = np.random.default_rng(1)
    ok = True

    def check(what, cond, got):
        nonlocal ok
        ok &= bool(cond)
        print(f"  [{'pass' if cond else 'FAIL'}] {what}: {got}")

    print("== selftest AFSK edges on a capture without tones (crashed on a bladeRF capture, Oct 27)")
    check("no switches: an empty result, no crash", afsk_edges(np.zeros(48_000), 48_000, 1.0, 1.0) == {}, "{}")

    print("== selftest 4FSK: +3 = 2400 Hz, clock +40 ppm, carrier +1.2 kHz, 6 s")
    raw = to_iq(gen_fsk4(6) * 800, 48_000, 40, 1200, rng)
    dev, fs, off = demod(raw, 0, 6000)
    r = eye(dev, fs, os.path.join(out_dir, "selftest-eye.png"))
    check("carrier offset", abs(off - 1200) < 30, f"{off:+.0f} Hz")
    check("levels within 2 % of +-800/+-2400", np.all(np.abs(r["levels"] - [-2400, -800, 800, 2400]) < 0.02 * 2400), np.round(r["levels"]))
    check("outer/inner ratio", abs(r["ratio"] - 3) < 0.05, f"{r['ratio']:.3f}")
    check("eyes open over 70 %", min(r["eyes"]) > 70, [round(e) for e in r["eyes"]])
    check("no symbol errors", r["errors"] == 0, r["errors"])

    print("== selftest sweep: a 300 Hz high-pass and 3 kHz low-pass (first order), 1 kHz = 1500 Hz, two passes")
    x = response(gen_sweep(2), 48_000, 300, 3000)
    x *= 1500 / (np.abs(1j * 1000 / 300 / (1 + 1j * 1000 / 300) / (1 + 1j * 1000 / 3000)))
    raw = to_iq(x, 48_000, -25, -800, rng)
    dev, fs, off = demod(raw, 0, 12_000)
    r = sweep(dev, fs, os.path.join(out_dir, "selftest-sweep.png"))
    check("1 kHz reference", abs(r["ref"] - 1500) < 30, f"{r['ref']:.0f} Hz")
    check("low -3 dB point (against 1 kHz, which the filter has at -0.6 dB: 253 Hz)", r["low"] and abs(r["low"] - 253) < 10, f"{r['low']:.0f} Hz")
    check("high -3 dB point (likewise, interpolated between 3150 and 4000 Hz: about 3.5 kHz)", r["high"] and abs(r["high"] - 3500) < 60, f"{r['high']:.0f} Hz")
    i80 = r["freqs"].index(8000)
    want = 20 * np.log10(np.abs(1j * 8000 / 300 / (1 + 1j * 8000 / 300) / (1 + 1j * 8000 / 3000)) / np.abs(1j * 1000 / 300 / (1 + 1j * 1000 / 300) / (1 + 1j * 1000 / 3000)))
    check("8 kHz within 0.1 dB of the filter", abs(r["db"][i80] - want) < 0.1, f"{r['db'][i80]:+.2f} vs {want:+.2f} dB")

    print("== selftest sweep, the capture starting 0.3 s into the key-up, 12 s of it (the first marker cut short)")
    x = gen_sweep(2)[int(0.3 * 48_000) : int(12.3 * 48_000)] * 3000
    dev, fs, off = demod(to_iq(x, 48_000, 10, 200, rng, lead=0.5), 0, 12_000, quiet=True)
    r = sweep(dev, fs)
    check("every tone measured, from the pass before the one whole marker", r and r["freqs"] == SWEEP_HZ, r and len(r["freqs"]))
    check("flat within 0.1 dB", r and max(abs(v) for v in r["db"]) < 0.1, r and f"{max(abs(v) for v in r['db']):.2f} dB")

    print("== selftest AFSK: mark 2500 Hz, space 2 dB hotter, 3 s")
    raw = to_iq(gen_afsk(3, 10 ** (2 / 20)) * 2500, 48_000, 10, 300, rng)
    dev, fs, off = demod(raw, 0, 6000)
    r = afsk(dev, fs)
    check("mark deviation", abs(r[1200] - 2500) < 75, f"{r[1200]:.0f} Hz")
    check("twist +2 dB", abs(r.get("twist", 99) - 2) < 0.4, f"{r.get('twist', 0):+.2f} dB")
    check("clean timing: jitter under 10 us rms", r.get("jitter_rms_us", 99) < 10, f"{r.get('jitter_rms_us', 0):.1f} us")
    check("baud: the +10 ppm clock", abs(r.get("baud_ppm", 99) - 10) < 20, f"{r.get('baud_ppm', 0):+.0f} ppm")
    pc = [r.get("phase_mark", (0, 99))[1], r.get("phase_space", (0, 99))[1]]
    check("phase-continuous: step spread under 10 deg", max(pc) < 10, pc)
    clean_spike = r.get("spike_hz", 0)

    print("== selftest AFSK, late switches (as the BK4829 path's late writes might be): each up to 300 us late, uniform")
    raw = to_iq(gen_afsk(3, 10 ** (2 / 20), jitter_us=300, rng=rng) * 2500, 48_000, -20, 300, rng)
    dev, fs, off = demod(raw, 0, 6000)
    r = afsk(dev, fs)
    want = 300 / np.sqrt(12)
    check(f"jitter near {want:.0f} us rms (uniform 0-300 us)", abs(r.get("jitter_rms_us", 0) - want) < 15, f"{r.get('jitter_rms_us', 0):.1f} us")
    check("baud: the -20 ppm clock", abs(r.get("baud_ppm", 99) + 20) < 40, f"{r.get('baud_ppm', 0):+.0f} ppm")

    print("== selftest AFSK, phase jumps: each tone run from a random phase, on time")
    raw = to_iq(gen_afsk(3, 10 ** (2 / 20), phase_jump=True, rng=rng) * 2500, 48_000, 0, 300, rng)
    dev, fs, off = demod(raw, 0, 6000)
    r = afsk(dev, fs)
    pc = [r.get("phase_mark", (0, 0))[1], r.get("phase_space", (0, 0))[1]]
    check("phase jumps: step spread over 40 deg", min(pc) > 40, [round(x) for x in pc])
    check("phase jumps: bigger spikes at the switches than the clean signal's", r.get("spike_hz", 0) > clean_spike * 1.2, f"{r.get('spike_hz', 0):.0f} vs {clean_spike:.0f} Hz")
    # Phase jumps move the detected switch instants too (each jump dents both tones' amplitudes differently), so the
    # jitter reads high on this signal although every switch is on time: read the jitter with the phase spread.
    print(f"  (jitter on a phase-jumping signal: {r.get('jitter_rms_us', 0):.0f} us rms, from the jumps, not the timing)")
    print("selftest:", "all pass" if ok else "FAILED")
    return ok


def main():
    a = argparse.ArgumentParser()
    a.add_argument("mhz", type=float)
    a.add_argument("--seconds", type=float, default=15)
    a.add_argument("--gain", type=float, default=0, help="tuner gain dB (0 = least; the radio is close)")
    a.add_argument("--ppm", type=int, default=0)
    a.add_argument("--save")
    a.add_argument("--load")
    a.add_argument("--skip", type=float, default=0.0, help="--eye: analyse from this many seconds after the preamble")
    a.add_argument("--tones", action="store_true")
    a.add_argument("--audio-lp", type=float, help="audio low-pass after the demodulator, Hz (6000; 12000 with --sweep)")
    a.add_argument("--eye", action="store_true", help="TERM-33 4FSK: levels, eyes, PRBS errors, drift; eye PNG")
    a.add_argument("--sweep", action="store_true", help="TERM-33 sweep: response, -3 dB points; plot PNG")
    a.add_argument("--afsk", action="store_true", help="TERM-33 AFSK flags (afsk or afsk-bk): deviations, twist, bit timing jitter, phase at the switches")
    a.add_argument("--out", help="where the PNGs go")
    a.add_argument("--selftest", action="store_true", help="run the analyses on synthetic IQ and check them")
    o = a.parse_args()
    global EYE_SKIP_S
    EYE_SKIP_S = o.skip
    if o.selftest:
        d = o.out or tempfile.mkdtemp(prefix="sdrtx-")
        sys.exit(0 if selftest(d) else 1)
    raw = np.fromfile(o.load, np.uint8) if o.load else capture(o)
    lp = o.audio_lp or (12_000 if o.sweep else 6000)
    d = demod(raw, o.mhz, lp)
    if d is None:
        return
    dev, fs, _ = d
    out = o.out or (os.path.dirname(os.path.abspath(o.load)) if o.load else os.getcwd())
    stem = os.path.splitext(os.path.basename(o.load))[0] if o.load else "sdrtx"
    if o.eye:
        eye(dev, fs, os.path.join(out, stem + "-eye.png"))
    if o.sweep:
        sweep(dev, fs, os.path.join(out, stem + "-sweep.png"))
    if o.afsk:
        print("AFSK:")
        afsk(dev, fs)
    if not (o.eye or o.sweep or o.afsk):
        spectrum_report(dev, fs, o.tones)


if __name__ == "__main__":
    main()
