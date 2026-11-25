"""POCSAG pages as NRZ audio (48 kHz WAV, unit level) for fmgen.py's `wav` mode: DEV=4500 makes it a pager signal.

  pocsaggen.py OUT.wav [--baud 512|1200|2400] [--capcode N] [--function 0-3] [--numeric] [--repeat N] TEXT

A bench test for DEC-4 and Pager mode (DEC-20): a known capcode and text the radio should list in Pages. Written
from the POCSAG standard (CCIR Radiopaging Code No. 1): 576 bits of preamble, then batches of the sync codeword
and 8 frames of two codewords; an address in the frame of its low 3 bits; BCH(31,21) and an even parity bit.

  python3 tools/bench-blade/pocsaggen.py /tmp/p.wav --capcode 1234567 "FERRITE PAGER TEST"
  DEV=4500 python3 tools/bench-blade/fmgen.py /tmp/p.sc16 wav /tmp/p.wav
  tools/bench-blade/bladetx.sh /tmp/p.sc16 439.9875
"""
import argparse
import wave

import numpy as np

AF = 48_000
SYNC = 0x7CD215D8
IDLE = 0x7A89C197
GEN = 0x769  # x^10 + x^9 + x^8 + x^6 + x^5 + x^3 + 1
NUMERIC = "0123456789*U -)("  # BCD digits as the standard maps them


def codeword(data21):
    """21 data bits -> the 32-bit codeword: data, BCH check bits, even parity."""
    r = data21 << 10
    for i in range(30, 9, -1):
        if r & 1 << i:
            r ^= GEN << (i - 10)
    cw = (data21 << 10 | r) << 1
    return cw | bin(cw).count("1") & 1


def message_bits(text, numeric):
    """The message's bits in transmission order, in 20-bit chunks."""
    bits = []
    if numeric:
        for ch in text:
            d = NUMERIC.index(ch)
            bits += [(d >> i) & 1 for i in range(4)]  # each digit LSB first
    else:
        for ch in text + "\x04":  # EOT ends an alpha page
            bits += [(ord(ch) >> i) & 1 for i in range(7)]  # each character LSB first
    while len(bits) % 20:
        bits.append(0 if not numeric else [0, 0, 1, 1][len(bits) % 4])  # alpha zero fill; numeric spaces (0xC, LSB first)
    return [int("".join(map(str, bits[i:i + 20])), 2) for i in range(0, len(bits), 20)]


def page(capcode, function, text, numeric):
    """The codewords after the preamble: sync, then frames, the address in frame capcode % 8."""
    words = [IDLE] * (2 * (capcode & 7))
    words.append(codeword((capcode >> 3) << 2 | function))
    words += [codeword(1 << 20 | m) for m in message_bits(text, numeric)]
    while len(words) % 16:
        words.append(IDLE)
    out = []
    for i in range(0, len(words), 16):
        out += [SYNC] + words[i:i + 16]
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("out")
    ap.add_argument("text")
    ap.add_argument("--baud", type=int, default=1200, choices=(512, 1200, 2400))
    ap.add_argument("--capcode", type=int, default=1234567)
    ap.add_argument("--function", type=int, default=3)
    ap.add_argument("--numeric", action="store_true")
    ap.add_argument("--repeat", type=int, default=1, help="pages, 1 s of silence between")
    ap.add_argument("--rate", type=int, default=AF, help="sample rate (48000 for fmgen.py; 9600 to check with upocsag)")
    a = ap.parse_args()
    assert 0 <= a.capcode < 1 << 21 and 0 <= a.function <= 3
    bits = [1, 0] * 288
    for w in page(a.capcode, a.function, a.text, a.numeric):
        bits += [(w >> (31 - i)) & 1 for i in range(32)]
    # a 1 is the lower frequency (NRZ, as pagers send it): -1
    spb = a.rate / a.baud
    one = -np.array(bits, float) * 2 + 1
    burst = one[(np.arange(int(len(bits) * spb)) / spb).astype(int)]
    gap = np.zeros(a.rate)
    audio = np.concatenate([x for _ in range(a.repeat) for x in (burst, gap)])
    with wave.open(a.out, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(a.rate)
        w.writeframes((audio * 30000).astype(np.int16).tobytes())
    print(f"{a.out}: {a.repeat} page(s), capcode {a.capcode}, {a.baud} baud, {len(audio) / a.rate:.1f} s")


main()
