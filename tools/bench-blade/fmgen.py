"""Test signals as FM IQ for the bladeRF (sc16q11 interleaved, 1 MS/s), the signal SHIFT Hz above the tuned centre
(away from the bladeRF's LO leak). Also writes the audio it modulates (48 kHz WAV) for the analysis.

  fmgen.py OUT.sc16 sweep                      # 1 kHz marker 0.5 s, then third-octave tones 50 Hz-8 kHz, 0.3 s each
  fmgen.py OUT.sc16 sweep12                    # the same on to 10 and 12.5 kHz (for the radio's `meter`)
  fmgen.py OUT.sc16 tone HZ SECONDS
  fmgen.py OUT.sc16 fsk BAUD DEV_HZ SECONDS    # 2-level NRZ, PRBS9, direct FM (POCSAG-style when DEV 4500)
  fmgen.py OUT.sc16 gmsk BAUD BT SECONDS       # GMSK (h 0.5: deviation baud/4)
  fmgen.py OUT.sc16 fsk4 SYM DEV_HZ SECONDS    # 4FSK, PRBS9 dibits (M17's map: 01 +3, 00 +1, 10 -1, 11 -3), RRC 0.5;
                                               # DEV_HZ the outer deviation (M17 2400 at 4800 sym)
  fmgen.py OUT.sc16 afsk BAUD MARK SPACE SECONDS   # continuous-phase AFSK, PRBS9 (Bell 202 at 1200 1200 2200)
  fmgen.py OUT.sc16 wav IN.wav                 # any 48 kHz mono audio (an SSTV picture from usstv's encode example)
Deviation for sweep/tone/afsk: DEV (env, default 2500 Hz peak).
"""
import os, sys, wave
import numpy as np

FS = 1_000_000
AF = 48_000
SHIFT = 200_000
DEV = float(os.environ.get("DEV", "2500"))
LEAD = 0.5  # unmodulated carrier first and last, s

THIRD = [50, 63, 80, 100, 125, 160, 200, 250, 315, 400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000, 5000, 6300, 8000]


def rrc(sps, beta, span=8):
    """Root raised cosine taps, SPS samples a symbol, unit peak response of the pair."""
    t = np.arange(-span * sps, span * sps + 1) / sps
    h = np.empty(len(t))
    for i, x in enumerate(t):
        if abs(x) < 1e-9:
            h[i] = 1 + beta * (4 / np.pi - 1)
        elif abs(abs(4 * beta * x) - 1) < 1e-9:
            h[i] = beta / np.sqrt(2) * ((1 + 2 / np.pi) * np.sin(np.pi / (4 * beta)) + (1 - 2 / np.pi) * np.cos(np.pi / (4 * beta)))
        else:
            h[i] = (np.sin(np.pi * x * (1 - beta)) + 4 * beta * x * np.cos(np.pi * x * (1 + beta))) / (np.pi * x * (1 - (4 * beta * x) ** 2))
    return h / np.sqrt(np.sum(h ** 2) * sps) * sps / np.sqrt(sps)


def prbs9(n):
    s, out = 0x1FF, np.empty(n, np.int8)
    for i in range(n):
        b = ((s >> 8) ^ (s >> 4)) & 1
        s = ((s << 1) | b) & 0x1FF
        out[i] = b
    return out


def tones(seq):
    """[(hz or 0, seconds)] -> audio at AF, unit peak."""
    parts = []
    for hz, sec in seq:
        t = np.arange(int(sec * AF)) / AF
        parts.append(np.sin(2 * np.pi * hz * t) if hz else np.zeros_like(t))
    return np.concatenate(parts)


def main():
    out, kind, *a = sys.argv[1:]
    if kind in ("sweep", "sweep12"):
        # sweep12: on to 10 and 12.5 kHz, the radio's `meter` bins (TERM-44)
        hzs = THIRD + ([10000, 12500] if kind == "sweep12" else [])
        seq = [(1000, 0.5), (0, 0.05)] + [x for hz in hzs for x in ((hz, 0.3), (0, 0.05))]
        freq_dev = tones(seq) * DEV
    elif kind == "tone":
        freq_dev = tones([(float(a[0]), float(a[1]))]) * DEV
    elif kind in ("fsk", "gmsk"):
        baud, sec = float(a[0]), float(a[2])
        bits = prbs9(int(baud * sec)) * 2 - 1
        spb = AF / baud
        idx = (np.arange(int(len(bits) * spb)) / spb).astype(int)
        nrz = bits[idx].astype(float)
        if kind == "fsk":
            freq_dev = nrz * float(a[1])
        else:
            bt = float(a[1])
            sig = np.sqrt(np.log(2)) / (2 * np.pi * bt) * spb
            k = np.arange(-int(2 * spb), int(2 * spb) + 1)
            g = np.exp(-k ** 2 / (2 * sig ** 2)); g /= g.sum()
            freq_dev = np.convolve(nrz, g, mode="same") * baud / 4
    elif kind == "fsk4":
        sym, sec = float(a[0]), float(a[2])
        b = prbs9(2 * int(sym * sec))
        lv = {(0, 1): 3, (0, 0): 1, (1, 0): -1, (1, 1): -3}
        syms = np.array([lv[(b[2 * i], b[2 * i + 1])] for i in range(len(b) // 2)], float)
        sps = AF / sym
        idx = (np.arange(int(len(syms) * sps)) / sps).astype(int)
        imp = np.zeros(len(idx)); imp[np.unique(idx, return_index=True)[1]] = syms  # one impulse a symbol
        freq_dev = np.convolve(imp, rrc(sps, 0.5), mode="same") * float(a[1]) / 3
        freq_dev *= float(a[1]) / np.percentile(np.abs(freq_dev), 99.5)  # outer level at DEV_HZ (peaks a little over)
    elif kind == "afsk":
        baud, mark, space, sec = float(a[0]), float(a[1]), float(a[2]), float(a[3])
        bits = prbs9(int(baud * sec))
        spb = AF / baud
        idx = (np.arange(int(len(bits) * spb)) / spb).astype(int)
        f = np.where(bits[idx] > 0, mark, space)
        freq_dev = np.sin(2 * np.pi * np.cumsum(f) / AF) * DEV
    elif kind == "wav":
        with wave.open(a[0], "rb") as w:
            assert w.getframerate() == AF and w.getnchannels() == 1 and w.getsampwidth() == 2, "48 kHz mono 16-bit"
            x = np.frombuffer(w.readframes(w.getnframes()), np.int16).astype(np.float64)
        freq_dev = x / max(np.abs(x).max(), 1) * DEV
    else:
        sys.exit(__doc__)
    lead = np.zeros(int(LEAD * AF))
    fd = np.concatenate([lead, freq_dev, lead])
    with wave.open(out + ".audio.wav", "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(AF)
        w.writeframes((fd / max(np.abs(fd).max(), 1) * 30000).astype(np.int16).tobytes())
    # to FS: linear interpolation of the instantaneous frequency, then integrate
    t = np.arange(int(len(fd) * FS / AF)) / FS
    fi = np.interp(t, np.arange(len(fd)) / AF, fd) + SHIFT
    ph = 2 * np.pi * np.cumsum(fi) / FS
    iq = np.empty(2 * len(ph), np.int16)
    # the bladeRF's I/Q is the mirror of the usual order (bench, Oct 27): Q first
    iq[0::2] = (np.sin(ph) * 1500).astype(np.int16)
    iq[1::2] = (np.cos(ph) * 1500).astype(np.int16)
    iq.tofile(out)
    print(f"{out}: {len(ph) / FS:.2f} s, {kind} {a}, dev {DEV:.0f}")


main()
