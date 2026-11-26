#!/usr/bin/env bash
# just remote RECIPE [ARGS]: copy this checkout to FERRITE_REMOTE (user@host:path) and run `just RECIPE ARGS` there.
# The checkout goes to path/ferrite; ../docs, ../testdata-private and ../testdata (each when present) to path/docs,
# path/testdata-private and path/testdata, so the remote finds the test data by the same rule (tools/testdata.sh).
# Not copied: target/, .git, .venv/, .env (the remote keeps its own). The remote needs just and cargo.
set -euo pipefail
cd "$(dirname "$0")/.."
[ $# -ge 1 ] || { echo "usage: just remote RECIPE [ARGS...]" >&2; exit 2; }
: "${FERRITE_REMOTE:?set FERRITE_REMOTE=user@host:path in .env (.env.example)}"
host=${FERRITE_REMOTE%%:*}
dir=${FERRITE_REMOTE#*:}
[ "$host" != "$FERRITE_REMOTE" ] && [ -n "$dir" ] || { echo "FERRITE_REMOTE: user@host:path, not $FERRITE_REMOTE" >&2; exit 2; }

ex=(--exclude /target/ --exclude .git --exclude /.venv/ --exclude /.env --exclude __pycache__/)
ssh "$host" "mkdir -p $dir"
rsync -az --delete "${ex[@]}" ./ "$host:$dir/ferrite/"
for s in docs testdata-private testdata; do
    if [ -d "../$s" ]; then rsync -az --delete "${ex[@]}" "../$s/" "$host:$dir/$s/"; fi
done
# the remote copy has no .git: the commit goes along, for the build stamp and the test reports
commit=$(git rev-parse --short=7 HEAD)
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then commit+=+; fi
args=$(printf ' %q' "$@")
exec ssh "$host" "cd $dir/ferrite && export PATH=\$HOME/.cargo/bin:\$PATH FERRITE_COMMIT=$commit &&
    if ! command -v just >/dev/null; then echo 'just is not installed on $host: cargo install just' >&2; exit 127; fi &&
    just$args"
