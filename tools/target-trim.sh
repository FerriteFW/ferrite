#!/bin/sh
# Keeps the host build cache in check: cargo never deletes old outputs, so target/debug grows with every change,
# feature set and tool build (211 GB by Nov 23, 2026). Past the limit it is removed whole; the next build or test run
# makes it again (a few minutes from cold, about 3 GB). Only target/debug: the images, reports, videos and the other
# folders in target/ stay. mkbtf.sh runs it before each build.
#
#   tools/target-trim.sh            # the limit is 25 GB (FERRITE_TARGET_LIMIT_GB to change it)
set -e
cd "$(dirname "$0")/.."
dir="${CARGO_TARGET_DIR:-target}/debug"
limit_gb="${FERRITE_TARGET_LIMIT_GB:-25}"
[ -d "$dir" ] && [ ! -L "$dir" ] || exit 0
kb=$(du -sk "$dir" | cut -f1)
if [ "$kb" -gt $((limit_gb * 1024 * 1024)) ]; then
    echo "target-trim: $dir is $((kb / 1024 / 1024)) GB, over $limit_gb GB: removing it (rebuilt on the next build)" >&2
    rm -rf -- "$dir"
fi
