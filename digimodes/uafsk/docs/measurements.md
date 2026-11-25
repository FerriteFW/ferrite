# Measurements

All figures are from `tests/report.rs` (`cargo test --release -p uafsk --test report -- --ignored --nocapture`); the
committed results are in [`bench/results/uafsk.json`](../../../bench/results/uafsk.json).

## Test data

The WA8LMF TNC Test CD (the testdata repo's `fetch/wa8lmf.sh`):
- **Track 1**: 40 minutes of APRS traffic on 144.39 MHz, flat audio (discriminator).
- **Track 2**: the same traffic, de-emphasised, as from a receiver's speaker.

Each track is tested at 44.1 kS/s (the CD) and resampled to 9600 S/s (what the radio's ADC runs at). The reference is
Dire Wolf 1.7's `atest` with its defaults, run in `tools/reference/Dockerfile`.

## Frames decoded

Correct FCS; in brackets, how many of them Dire Wolf also decoded.

| Configuration | Track 1, 9.6k | Track 1, 44.1k | Track 2, 9.6k | Track 2, 44.1k |
| --- | --- | --- | --- | --- |
| Dire Wolf `atest` | 993 | 1005 | 953 | 982 |
| `<1, 1>` one slicer | 990 (976) | 1003 (992) | 981 (939) | 996 (975) |
| `<3, 1>` no high-pass | 1002 (979) | 1007 (995) | 961 (932) | 972 (965) |
| **`<3, 1>` default** | **997 (981)** | **1007 (996)** | **998 (945)** | **1009 (980)** |
| `<3, 2>` both chains | 1006 (983) | 1011 (998) | 999 (945) | 1009 (980) |
| `<4, 1>` with the raw slicer | 999 (981) | 1009 (997) | 998 (945) | 1009 (980) |
| Default without repair (`repair: 0`) | 986 (973) | 988 (984) | 978 (941) | 990 (977) |
| One-bit window, PLL 74% | 938 (931) | 956 (955) | 943 (919) | 958 (953) |

The frames uafsk decodes that Dire Wolf does not (11 on track 1, 43 on track 2 at 9.6k with the default) all carry
valid AX.25 addresses.

## Off-air recordings

`recordings` in `tests/report.rs`: any `aprs/NAME.wav` in the private test data with Dire Wolf's `atest -h`
output in `aprs/ref/NAME.atest.txt`. Two recordings of a busy APRS channel at 16 kS/s, not redistributed; the
radio runs 12 kS/s, so each is also scored resampled to 12 kS/s (ffmpeg). Frames (also Dire Wolf's):

| | Rec. 1, 12k | Rec. 1, 16k | Rec. 2, 12k | Rec. 2, 16k |
| --- | --- | --- | --- | --- |
| Dire Wolf `atest` (16k) | 353 | 353 | 367 | 367 |
| `<1, 1>` one slicer | 355 (322) | 368 (326) | 373 (363) | 377 (363) |
| **`<3, 1>` default** | **393 (333)** | **406 (336)** | **386 (363)** | **389 (365)** |
| `<3, 2>` both chains | 401 (336) | 409 (336) | 393 (364) | 396 (366) |
| Default without repair | 360 (319) | 368 (323) | 379 (362) | 376 (361) |
| One-bit window | 205 (203) | 205 (204) | 332 (332) | 336 (336) |

Recording 1 resampled to 48 kS/s: 374 (327), previously 236. All the frames only uafsk decodes pass AX.25 address
checks; most are Mic-E position reports with a telemetry counter, so they are distinct transmissions.

### Repair

Of the 39 frames Dire Wolf decoded and uafsk (without repair) missed at 12 kS/s, 37 were closed by one of the
slicers with a bad FCS, most with exactly two adjacent wrong data bits: one wrong tone decision, anywhere in the
frame. Other windows, clock rates and tone frequencies slip on the same symbols (their unions add only 3 to 8
frames); Dire Wolf slips on different ones. So the default now repairs: each tone decision carries its margin, the
deframer keeps the least confident few, and a frame with a bad FCS gets those flipped (all combinations, fewest
first). Scored at 12 kS/s (frames, in brackets also Dire Wolf's):

| Least confident decisions tried (`Config::repair`) | Rec. 1 | Rec. 2 | Combinations per bad frame |
| --- | --- | --- | --- |
| 0 (off) | 360 (319) | 379 (362) | 0 |
| 1 | 374 (327) | 382 (363) | 1 |
| 2 | 385 (330) | 382 (363) | 3 |
| **3 (the default)** | **393 (333)** | **386 (363)** | **7** |
| 4 | 402 (336) | 388 (363) | 15 |

What the repaired frames are, with 3 (recordings 1 and 2): 56 and 13 repaired frames reported; 12 and 1 are frames
Dire Wolf decoded that uafsk had missed; 27 and 5 are frames uafsk also had from another slicer; 17 and 7 are not in
Dire Wolf's output. Those 24 all pass AX.25 address checks, and none disagrees with a copy decoded within a second
of it: the copies nearby are the same frame or its digipeated repeat (different path). Most are Mic-E position
reports with a changing telemetry counter. None reads as garbage. For comparison, flipping every symbol in turn
(not just the least confident) on recording 1 recovered 18 missed frames but also 17 unverifiable ones and 3
garbage frames, over about 600 tries a frame.

How often a repair could be false: a wrong combination passes a CRC-16 by chance about one time in 65,536, so with
7 tries one bad frame in about 9,400. On 400 million random bits (57,498 closures with a bad FCS) repair 3 let 4
frames through and repair 4 let 11 through before the address check, close to that rate; the address check stopped
all of them. The recordings close about 1,000 frames an hour with a bad FCS (white or filtered noise: about 370), so
a false repair with plausible addresses would be about one every 10 hours on a busy channel; three hours of
noise (white, filtered, in bursts) gave none. `recordings_crc` recomputes the FCS of every repaired frame independently (125 and 32 at 12 kS/s): 0 wrong.
A frame is repaired only if the bit stuffing stays the same after the flips, and the first combination that fixes the
FCS is the only one taken.

Cost: 60 bytes of RAM (20 per deframer: the 3 candidates and the previous decision) and about 3% of the CPU
(bookkeeping of each decision while a frame is open). The repair itself runs only when a frame fails its FCS: a few
thousand instructions for a typical frame, at most about 40,000 for a 330-byte one (0.25 ms of a 240 MHz core),
about once every 4 seconds on a busy channel; it is part of the averages below.

### Other changes tried

What was tried, scored at 12 kS/s on both recordings (default otherwise):

| Change | Rec. 1 | Rec. 2 | WA8LMF 1 / 2, 9.6k |
| --- | --- | --- | --- |
| Previous default | 205 | 332 | 938 / 943 |
| Window 1.2 bits (PLL 74%, as before) | 276 | 354 | 979 / 961 |
| Window 1.3 bits | 332 | 368 | 979 / 961 |
| Window 1.4 bits | 353 | 374 | 983 / 975 |
| Window 1.5 bits | 331 | 367 | 986 / 983 |
| Window 1.7 bits | 291 | 353 | 959 / 962 |
| 1.4 bits and PLL 85% (**the default**) | **360** | **379** | **986 / 978** |
| ... with PLL 60% while searching (no carrier) | 358 | 379 | 985 / 977 |
| ... with a band-pass prefilter (1700 Hz, Q 0.7) after the high-pass | 356 | 374 | 988 / 985 |
| ... band-pass (Q 1.7) instead of the high-pass | 295 | 371 | 970 / 970 |
| ... tone amplitudes smoothed (one pole, 1/2) | 355 | 374 | 993 / 981 |
| ... a second 0.3-bit window (trapezoid; PLL 74%) | 354 | 370 | 992 / 982 |
| ... normaliser decay 150 / 350 ms (250) | 361 / 358 | 377 / 376 | 982 / 985, 985 / 976 |
| ... normaliser attack 0.5 (0.7) | 360 | 378 | 984 / 979 |
| ... slicer gains 0, -6, +6 dB (PLL 74%) | 337 | 365 | 971 / 977 |
| ... slicer gains 0, -2, +2 dB | 354 | 375 | 979 / 976 |
| ... high-pass corner 800 / 1600 Hz (1200) | 348 / 359 | 374 / 380 | 988 / 979, 983 / 979 |
| ... tapered window: the 1.4-bit sums summed again over 0.2 / 0.4 / 0.6 bit (trapezoid) | 358 / 350 / 336 | 373 / 369 / 368 | |
| ... tapered, same total length: 1.1 + 0.3 bits / 1.2 + 0.2 bits | 278 / 303 | 354 / 362 | |
| ... a one-pole smoother on the I/Q sums (1/2, 1/4) | 353 / 312 | 371 / 358 | |
| ... decide from the margin summed over the middle 0.2 / 0.3 / 0.4 bit (not one sample) | 371 / 372 / 366 | 377 / 374 / 376 | |
| ... the same (0.3 bit) with repair 3 | 414 (338) | 393 (365) | |

A tapered window lowers the sidelobes (a boxcar's first is -13 dB, about where the other tone falls; a triangle's
-27 dB) but widens the main lobe for the same length, letting in more noise; and lengthening it to keep the main
lobe blurs neighbouring bits. Every taper lost frames. Summing the margin over the middle of the bit adds 5 frames
on recording 1 with repair (338 against 333 of Dire Wolf's) and 2 on recording 2, but costs 28% more CPU (430
instructions a sample against 335): not kept.

## Cost

| | `<1, 1>` | `<3, 1>` | `<3, 2>` |
| --- | --- | --- | --- |
| RAM | about 0.6 KB | 1,348 B | about 2.6 KB |

CPU of `<3, 1>` at 9600 S/s, counted in a Cortex-M4 emulator (`tools/cpu-bench`): 3.3 million instructions a second
(345 a sample; 334 with `repair: 0`), 2.1% of a 240 MHz core, 4.1% at 120 MHz; at 12000 S/s, about 4.1 million. The window's length costs
nothing per sample, only its ring (2 bytes a sample). Cost scales with the sample rate, so 9600 S/s is the right rate for a radio.

## Carrier detect

`carrier_detect` in `tests/report.rs`; `Demodulator<3, 1>` at 9600 S/s.

| Audio | Frames | Busy as a frame is decoded | Busy during frames | Busy away from any frame |
| --- | --- | --- | --- | --- |
| Track 1 | 997 | 100% | 99.9% | 16.8% |
| Track 2 | 998 | 100% | 99.9% | 17.6% |

"Away from any frame" excludes half a second around every frame uafsk or Dire Wolf decoded. What remains busy
comes in stretches with a median of 105-170 ms and up to about 1 s, the length of packets, so most of it is
probably collisions and packets neither decoder gets. On synthetic audio that is not packet (60 s each of white
noise, silence, steady 1200, 2200 and 1000 Hz tones, and a voice-like sweep) it is busy 0% of the time. A steady
tone between the two ripples into transitions that look like data at some frequencies (1000 and 1300 Hz with the
1.4-bit window), so carrier detect also needs both tones to come and go (each one's valley under half its peak). A
steady 600 Hz tone still reads as busy, as it did before the change. In a clean synthetic packet it turns on 93 ms into the preamble (about 15 flags) and stays on to the
end.
