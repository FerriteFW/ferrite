#!/bin/sh
# Fuzz the packet decoders (digimodes/FUZZING.md): APRS parsing, AX.25 frames, KISS, FX.25, HDLC, AFSK audio and the
# radio's receive path. Host only, nightly (the pinned toolchain), cargo-fuzz (`cargo install cargo-fuzz --locked`).
#
#   tools/fuzz.sh [SECONDS] [TARGET...]    each target for SECONDS (default 60), one after another
#   tools/fuzz.sh list                     the targets
#   tools/fuzz.sh seeds                    rebuild the seed corpora only
#
# Everything lands under target/fuzz/: corpus/TARGET (kept between runs, grows), artifacts/ (crashes, minimise with
# `cargo fuzz tmin --target-dir target/fuzz TARGET FILE`), logs/. JOBS (default: half the cores) runs that many
# workers per target. Exits 1 if any target found a crash.
set -eu
cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="$ROOT/target/fuzz"
TARGETS="aprs_parse ax25_frame kiss fx25_block fx25_bits hdlc_bits afsk_audio aprs_live"
TESTDATA=$(tools/testdata.sh) # TESTDATA, ../testdata-private or ../testdata

seeds() {
    set -- "$ROOT/digimodes/uaprs/tests/spec_packets.txt"
    for f in "$TESTDATA"/aprs/*.hex "$TESTDATA"/fx25/msgs.txt; do
        [ -f "$f" ] && set -- "$@" "$f"
    done
    (cd fuzz && CARGO_TARGET_DIR="$OUT" cargo run -q --release --bin seeds -- "$OUT/corpus" "$@")
}

case "${1:-}" in
    list) echo "$TARGETS"; exit 0 ;;
    seeds) seeds; exit 0 ;;
esac

SECONDS_EACH=${1:-60}
[ $# -gt 0 ] && shift
[ $# -gt 0 ] && TARGETS="$*"
JOBS=${JOBS:-$(( $(sysctl -n hw.ncpu 2>/dev/null || nproc) / 2 ))}
[ "$JOBS" -lt 1 ] && JOBS=1

mkdir -p "$OUT/logs" "$OUT/artifacts"
[ -d "$OUT/corpus" ] || seeds
cargo fuzz build --target-dir "$OUT" 2>&1 | grep -E '^(error|warning)' || true

failed=""
for t in $TARGETS; do
    # an AFSK frame at 24 kHz takes 40 bytes of input a bit
    max=8192
    [ "$t" = afsk_audio ] && max=65536
    mkdir -p "$OUT/corpus/$t"
    echo "fuzz $t: ${SECONDS_EACH}s, $JOBS jobs"
    if ! cargo fuzz run --target-dir "$OUT" "$t" "$OUT/corpus/$t" -- \
        -max_total_time="$SECONDS_EACH" -max_len=$max -timeout=10 -rss_limit_mb=2048 \
        -fork="$JOBS" -ignore_crashes=0 -artifact_prefix="$OUT/artifacts/$t-" -print_final_stats=1 \
        > "$OUT/logs/$t.log" 2>&1; then
        failed="$failed $t"
    fi
    grep -E '^#[0-9]+.*(DONE|cov:)' "$OUT/logs/$t.log" | tail -1 || true
done
if [ -n "$failed" ]; then
    echo "crashes:$failed (target/fuzz/artifacts, logs in target/fuzz/logs)"
    exit 1
fi
echo "ok: no crashes"
