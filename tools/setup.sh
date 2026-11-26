#!/usr/bin/env bash
# just setup: the in-repo .venv, the pinned Rust toolchain, cargo-nextest; then what is here.
# just doctor (tools/setup.sh --check): only checks the versions against the pins, installs nothing.
set -uo pipefail
cd "$(dirname "$0")/.."
check=${1:-}
NEXTEST=0.9.151 # as CI (.forgejo/workflows/release.yml)
PY_MIN=$(cat .python-version)
TOOLCHAIN=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
bad=0
ok() { printf '  ok       %s\n' "$*"; }
no() { printf '  MISSING  %s\n' "$*"; bad=1; }
note() { printf '  -        %s\n' "$*"; }

echo "tools"
py=python3
command -v "python$PY_MIN" >/dev/null && py="python$PY_MIN"
if ! command -v $py >/dev/null; then
    no "python3 (>= $PY_MIN)"
elif ! $py -c "import sys; sys.exit(sys.version_info < tuple(map(int, '$PY_MIN'.split('.'))))"; then
    no "python3 >= $PY_MIN (found $($py --version 2>&1))"
else
    ok "$($py --version 2>&1) (>= $PY_MIN, .python-version)"
    if [ -z "$check" ] && [ ! -x .venv/bin/python ]; then
        $py -m venv .venv || no ".venv: python3 -m venv failed"
    fi
    if [ -x .venv/bin/python ]; then
        [ -z "$check" ] && .venv/bin/python -m pip install -q --disable-pip-version-check -r tools/requirements.txt
        missing=$(.venv/bin/python - <<'PY'
import importlib.util
mods = {"pyserial": "serial", "bleak": "bleak", "numpy": "numpy", "matplotlib": "matplotlib", "Pillow": "PIL",
        "fonttools": "fontTools", "segno": "segno", "Markdown": "markdown"}
print(" ".join(k for k, m in mods.items() if importlib.util.find_spec(m) is None))
PY
)
        if [ -n "$missing" ]; then no ".venv lacks: $missing (just setup)"; else ok ".venv (tools/requirements.txt)"; fi
    else
        no ".venv (just setup)"
    fi
fi

if ! command -v rustup >/dev/null; then
    no "rustup (https://rustup.rs)"
else
    [ -z "$check" ] && rustup toolchain install >/dev/null 2>&1 # reads rust-toolchain.toml, as CI does
    if rustup toolchain list | grep -q "^$TOOLCHAIN"; then ok "Rust $TOOLCHAIN (rust-toolchain.toml)"; else no "Rust $TOOLCHAIN (just setup)"; fi
fi

have=$(cargo nextest --version 2>/dev/null | head -1 | awk '{print $2}')
if [ "$have" != "$NEXTEST" ] && [ -z "$check" ] && command -v cargo >/dev/null; then
    case "$(uname -s)" in Darwin) os=mac ;; *) os=linux ;; esac
    mkdir -p "$HOME/.cargo/bin"
    curl -LsSf "https://get.nexte.st/$NEXTEST/$os" | tar zxf - -C "$HOME/.cargo/bin" && \
        have=$(cargo nextest --version 2>/dev/null | head -1 | awk '{print $2}')
fi
if [ "$have" = "$NEXTEST" ]; then ok "cargo-nextest $NEXTEST"; elif [ -n "$have" ]; then no "cargo-nextest $NEXTEST (found $have; just setup)"; else no "cargo-nextest $NEXTEST (just setup)"; fi

if command -v arm-none-eabi-readelf >/dev/null; then
    ok "ARM binutils (the fast-flash check, the size tools)"
else
    no "ARM binutils: brew install arm-none-eabi-binutils / apt install binutils-arm-none-eabi (DEVELOPING.md)"
fi
if command -v just >/dev/null; then ok "$(just --version)"; else no "just (brew install just, cargo install just)"; fi

echo "optional"
if [ -d ../docs ]; then ok "../docs"; else note "../docs not found: recipes that need it say skipped"; fi
td=$(tools/testdata.sh)
if [ -d "$td" ]; then ok "test data: $td"; else note "test data not found ($td): clone FerriteFW/testdata beside ferrite (fetch/all.sh for the rest); tests that need it say skipped"; fi
[ -f .env ] && set -a && . ./.env && set +a
if [ -n "${FERRITE_REMOTE:-}" ]; then ok "FERRITE_REMOTE=$FERRITE_REMOTE (just remote RECIPE)"; else note "FERRITE_REMOTE not set (.env.example)"; fi
radios=${FERRITE_RADIOS:-$HOME/.config/ferrite/radios.toml}
if [ -f "$radios" ]; then ok "$radios"; else note "no $radios (radios.example.toml; just radio-add NAME)"; fi
exit $bad
