"""Squelch test signals as FM IQ for the bladeRF (sc16q11, 1 MS/s, 200 kHz above the tuned centre, as fmgen.py).

  sqgen.py OUT.sc16 SECONDS [--af HZ] [--ctcss HZ] [--dcs CODE] [--inv] [--gap MS --every S] [--tail55 MS]
                            [--lead S] [--trail S] [--db DB]

Carrier on for SECONDS, with a 1 kHz tone (--af, 0 for a bare carrier, 2.5 kHz deviation), and a subtone:
CTCSS at 500 Hz deviation, or DCS (octal CODE, 134.4 bit/s, the 23-bit word sent LSB first as drivers/bk4829's
dcs_codeword, `--inv` the complement) at 600 Hz. `--gap MS --every S` drops the subtone alone for MS ms every
S seconds (a fade on the tone, carrier and voice kept). `--tail55 MS`: the last MS ms of the carrier carry a 55 Hz
tone in place of the subtone (stock's tail). --lead / --trail: no carrier before and after (silence in the file).
"""
import argparse
import numpy as np

FS = 1_000_000
SHIFT = 200_000


def dcs_word(code, inv):
    data = 0x800 | (code & 0x1FF)
    r = data << 11
    for i in range(22, 10, -1):
        if r >> i & 1:
            r ^= 0xC75 << (i - 11)
    w = data | r << 12
    return (~w & 0x7FFFFF) if inv else w


def main():
    a = argparse.ArgumentParser()
    a.add_argument("out")
    a.add_argument("seconds", type=float)
    a.add_argument("--af", type=float, default=1000)
    a.add_argument("--ctcss", type=float)
    a.add_argument("--dcs", type=lambda s: int(s, 8))
    a.add_argument("--inv", action="store_true")
    a.add_argument("--gap", type=float, default=0)
    a.add_argument("--every", type=float, default=1.0)
    a.add_argument("--tail55", type=float, default=0)
    a.add_argument("--lead", type=float, default=0.2)
    a.add_argument("--trail", type=float, default=0.5)
    a.add_argument("--db", type=float, default=0, help="the signal's level, dB from full (weaker for squelch thresholds)")
    o = a.parse_args()
    assert o.dcs is None or 0 <= o.dcs <= 0o777
    if o.dcs is not None:
        assert dcs_word(0o23, False) == 0x763813

    n = int(o.seconds * FS)
    t = np.arange(n) / FS
    fm = np.zeros(n)
    if o.af:
        fm += 2500 * np.sin(2 * np.pi * o.af * t)
    sub = np.zeros(n)
    if o.ctcss:
        sub = 500 * np.sin(2 * np.pi * o.ctcss * t)
    elif o.dcs is not None:
        w = dcs_word(o.dcs, o.inv)
        bits = np.array([(w >> i) & 1 for i in range(23)])
        idx = (t * 134.4).astype(int) % 23
        nrz = bits[idx] * 2.0 - 1.0
        k = int(FS / 300)  # crude low-pass: a moving average about a 300 Hz cutoff
        nrz = np.convolve(nrz, np.ones(k) / k, mode="same")
        sub = 600 * nrz
    if o.gap:
        g = (t % o.every) < o.gap / 1000
        sub[g] = 0
    if o.tail55:
        m = t >= o.seconds - o.tail55 / 1000
        sub[m] = 500 * np.sin(2 * np.pi * 55 * t[m])
    fm += sub
    ph = 2 * np.pi * np.cumsum(fm + SHIFT) / FS
    iq = np.exp(1j * ph) * 0.7 * 10 ** (o.db / 20)
    lead, trail = (np.zeros(int(x * FS), complex) for x in (o.lead, o.trail))
    iq = np.concatenate([lead, iq, trail])
    s = np.empty(2 * len(iq), np.int16)
    s[0::2] = np.round(iq.real * 2047)
    s[1::2] = np.round(iq.imag * 2047)
    s.tofile(o.out)


if __name__ == "__main__":
    main()
