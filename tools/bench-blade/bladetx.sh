#!/bin/zsh
XEON=${FERRITE_XEON:-xeon} # the box with the bladeRF (an ssh host)
# bladetx.sh FILE.sc16 [MHZ] [REPEAT]: send the file from the Xeon's bladeRF so its signal lands on MHZ. The file's
# signal is 200 kHz above its centre, but the bladeRF sends the mirror (as bladecap.sh swaps I and Q), so it is tuned
# 200 kHz above MHZ (Nov 16, 2026). TXVGA1 (env, default -35 dB; -25 gives about S8 on black at 60 cm).
set -eu
f=$(python3 -c "print(int(round(${2:-446.475}*1e6))+200000)")
scp -q $1 $XEON:/tmp/tx.sc16
ssh $XEON "bladeRF-cli -e 'set frequency tx $f; set samplerate tx 1000000; set bandwidth tx 1500000; set txvga1 ${TXVGA1:--35}; set txvga2 0; tx config file=/tmp/tx.sc16 format=bin repeat=${3:-1}; tx start; tx wait' 2>&1 | grep -i -E 'error|fail' || true"
