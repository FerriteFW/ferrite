#!/bin/sh
# Build and pack a Ferrite image for the stock bootloader, then check it with the flasher's dry run.
#
#   tools/mkbtf.sh            # target/ferrite.btf, the normal receive-only build (a user build: no bench commands)
#   tools/mkbtf.sh bringup    # target/ferrite-bringup.btf: no SPI flash writes, key test on a held key, cable log,
#                             #   the bench commands
#   tools/mkbtf.sh tx         # target/ferrite-tx.btf: the transmit build (a user build: no bench commands)
#   tools/mkbtf.sh tx-bench clock-240   # any build at another fixed core clock (clock-24/48/96/144/192/240, power
#                             #   bench): target/ferrite-txbench-240.btf
#   tools/mkbtf.sh tx-bench   # target/ferrite-txbench.btf: the full-power transmit build with the bench commands
#                             #   (txtest, bk, si, mem, gpio, rx, mic, bt, gps query/nmea/standby), starting every
#                             #   power-on with Dry TX on (TX-48.1; decision 193 replaced the tx-dry build)
#   tools/mkbtf.sh reader     # target/ferrite-reader.btf: the read-only reader image Ferrite Console backs a radio
#                             #   up with (reader/, decision 264; replaced fw/apps/hwtest's). All of it runs from the
#                             #   zero-wait flash (reader/memory.x checks), so no fast-flash audit.
#   tools/mkbtf.sh restorer   # target/ferrite-restorer.btf: Back to stock for a radio not running Ferrite
#                             #   (restorer/, BACKUP-13): writes the backup back inside WRITABLE only.
#
# The bench commands are the `bench` cargo feature (decision 151, TERM-38): the firmware's `bringup` and `tx-bench`
# features turn it on (firmware/Cargo.toml). The receive and tx images leave them out; their terminal says "not in
# this build: a bench build has it".
#
# Each Ferrite build also packs target/NAME.ffw (decision 335): the .btf and its SPI parts (text pack, logo, terminal
# templates), unsigned. Flash with: python3 tools/flash.py PORT target/ferrite-tx.ffw --backup flash.bin --dev-unsigned
set -e
cd "$(dirname "$0")/.."
tools/target-trim.sh # the host build cache, capped (it grew to 211 GB)
name=ferrite
build="fw"
elf=ferrite
case "$1" in
    "") ;;
    bringup) name=ferrite-bringup build="fw-bringup" ;;
    tx) name=ferrite-tx build="fw --features tx" ;;
    tx-bench) name=ferrite-txbench build="fw --features tx-bench" ;; # tx + bench, Dry TX on at power-on
    reader) name=ferrite-reader build="fw-reader" elf=ferrite-reader ;;
    restorer) name=ferrite-restorer build="fw-restorer" elf=ferrite-restorer ;; # Back to stock without Ferrite (BACKUP-13)
    *) echo "usage: $0 [bringup | tx | tx-bench | reader | restorer] [clock-24 | clock-48 | clock-96 | clock-144 | clock-192 | clock-240 | no-vt100]" >&2; exit 2 ;;
esac
case "$2" in
    "") ;;
    clock-24|clock-48|clock-96|clock-144|clock-192|clock-240)
        name="$name-${2#clock-}"
        case "$build" in *--features*) build="$build,$2" ;; *) build="$build --features $2" ;; esac ;;
    no-vt100)
        # decision 342: without the terminal's full-screen pages (the command line stays), to compare sizes
        name="$name-novt100"
        case "$build" in *--features*) build="$build,sat-predict --no-default-features" ;; *) build="$build --features sat-predict --no-default-features" ;; esac ;;
    *) echo "unknown clock or variant: $2" >&2; exit 2 ;;
esac
[ "$elf" != ferrite ] && [ -n "$2" ] && { echo "the reader and the restorer have no clock builds" >&2; exit 2; }
cargo $build
if [ "$elf" = ferrite ]; then
    # The fast-flash audit must pass: its last line, or the whole report and a stop when it fails.
    check=$(python3 tools/slowcheck.py) || { echo "$check"; exit 1; }
    echo "$check" | tail -1
fi
bin=$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/host: //p')/bin
if [ "$elf" != ferrite ]; then
    # BACKUP-9.6: the reader's stack is what static memory leaves of the 96 KB. Too little and it faults at start-up,
    # and a radio stuck there needs its battery pulled to reach the bootloader (Oct 23). Keep 20 KB at least.
    used=$("$bin/llvm-size" -A "target/thumbv7em-none-eabihf/release/$elf" | awk '$1==".data"||$1==".bss"||$1==".uninit"{s+=$2} END{print s+0}')
    left=$((96 * 1024 - used))
    [ "$left" -ge $((20 * 1024)) ] || { echo "$elf: static RAM $used bytes leaves $left for the stack (need 20480)" >&2; exit 1; }
    echo "$elf RAM: $used static, $left stack"
fi
"$bin/llvm-objcopy" -O binary "target/thumbv7em-none-eabihf/release/$elf" "target/$name.bin"
python3 tools/btf.py pack "target/$name.bin" "target/$name.btf"
python3 tools/flash.py --dry-run - "target/$name.btf"
shasum -a 256 "target/$name.btf"
# The update file (decision 335, UPD-20): the image and its SPI parts in one .ffw beside the .btf, unsigned (CI
# signs releases). The parts carry the image's build ID, read back from the image itself.
if [ "$elf" = ferrite ]; then
    bid=$(python3 tools/ffw.py build-id "target/$name.bin")
    rm -rf "target/parts/$name"
    cargo run -q -p ferrite-sim --bin ferrite-mkparts -- --build "$bid" "target/parts/$name" > /dev/null
    ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
    kind=${name#ferrite}; kind=${kind#-}
    python3 tools/ffw.py pack "target/$name.ffw" --btf "target/$name.btf" --parts "target/parts/$name" \
        --kind "${kind:-rx}" --build "$bid" --version "$ver"
    python3 tools/ffw.py check "target/$name.ffw" --dev-unsigned > /dev/null
fi
# The size against the last recorded build; on a clean main it records this one (tools/size-history.tsv).
if [ "$elf" = ferrite ]; then python3 tools/sizelog.py "$name"; fi
