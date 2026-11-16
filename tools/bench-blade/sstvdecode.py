"""SSTV from a bench capture: a bladecap.sh (or rtl_sdr) file, 1.024 MS/s u8 IQ, the signal near +250 kHz (found by
its peak), FM-demodulated to 24 kHz and decoded by usstv's `live` example into OUT.bmp (and OUT.png).

  sstvdecode.py CAPTURE.u8 OUT.bmp [VIS]
Prints the picture's mode and lines, and the sync jitter: how far each line's sync lands from a straight line
(the sender's timing; a steady sender is well under 1 sample at 24 kHz)."""
import subprocess, sys, os
import numpy as np

FS = 1_024_000
HERE = os.path.dirname(os.path.abspath(__file__))
LIVE = os.path.join(HERE, "../../target/release/examples/live")


def audio(path):
    u = np.fromfile(path, np.uint8).astype(np.float32) - 127.5
    z = u[0::2] + 1j * u[1::2]
    s = z[len(z) // 2: len(z) // 2 + 65536]
    sp = np.abs(np.fft.fft(s * np.hanning(len(s))))
    f0 = np.fft.fftfreq(len(s), 1 / FS)[sp.argmax()]
    z = z * np.exp(-2j * np.pi * f0 * np.arange(len(z)) / FS)
    # a real channel filter before the discriminator: 8:1 by averaging (128 kS/s), then a windowed-sinc low-pass at
    # 9 kHz and 4:1 (32 kS/s). The averaging alone let the bladeRF's mirror image of the carrier (500 kHz away)
    # fold in near 12 kHz and beat with the signal: lines at 5-11 kHz and a ripple in the picture (Nov 15).
    y = z[: len(z) // 8 * 8].reshape(-1, 8).mean(axis=1)
    k = np.arange(-63, 64)
    h = np.sinc(2 * 9000 / 128000 * k) * np.blackman(len(k))
    h /= h.sum()
    y = np.convolve(y, h, mode="same")[::4]
    hz = np.angle(y[1:] * np.conj(y[:-1])) * 32000 / (2 * np.pi)
    t = np.arange(int(len(hz) * 24000 / 32000)) / 24000
    return np.interp(t, np.arange(len(hz)) / 32000, hz), f0


def jitter(a, period_s):
    """Sync starts (falling below 1350 Hz-equivalent after the tone mapping is unknown): use the audio's own
    instantaneous frequency in 1 ms steps and find 1200 Hz runs."""
    fs = 24000
    x = a - a.mean()
    # instantaneous tone frequency by zero crossings in a 2 ms sliding window
    zc = np.where(np.diff(np.signbit(x)))[0]
    tone = np.zeros(len(x))
    if len(zc) > 2:
        d = np.diff(zc)
        f = fs / (2 * np.maximum(d, 1))
        tone[zc[1:]] = f
        idx = np.nonzero(tone)[0]
        tone = np.interp(np.arange(len(x)), idx, tone[idx])
    low = tone < 1350
    starts = np.nonzero(low[1:] & ~low[:-1])[0]
    # keep the syncs a period apart
    keep, last = [], -10**9
    for s in starts:
        run = low[s:s + int(0.004 * fs)].mean()
        if run > 0.8 and s - last > 0.8 * period_s * fs:
            keep.append(s)
            last = s
    keep = np.array(keep, float)
    if len(keep) < 10:
        return None
    n = np.round((keep - keep[0]) / (period_s * fs))
    p = np.polyfit(n, keep, 1)
    r = keep - np.polyval(p, n)
    return len(keep), np.std(r), np.max(np.abs(r)), p[0] / fs


def main():
    cap, out = sys.argv[1], sys.argv[2]
    a, f0 = audio(cap)
    pcm = np.clip(a * 6, -32767, 32767).astype("<i2").tobytes()
    r = subprocess.run([LIVE, "24000", out] + sys.argv[3:4], input=pcm, capture_output=True)
    print(r.stderr.decode().strip())
    print(open(out + ".txt").read().strip(), f"(carrier at {f0 / 1e3:+.1f} kHz)")
    subprocess.run(["sips", "-s", "format", "png", out, "--out", out.rsplit(".", 1)[0] + ".png"], capture_output=True)
    j = jitter(a, 0.150)
    if j:
        print(f"syncs {j[0]}, timing jitter rms {j[1]:.2f} samples, worst {j[2]:.1f} (24 kHz), line period {j[3] * 1e3:.3f} ms")


main()
