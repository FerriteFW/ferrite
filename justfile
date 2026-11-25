# Ferrite's everyday commands: `just --list`. First time: `just setup`.
# The recipes call the scripts in tools/ and cargo; CI calls those scripts directly.

set dotenv-load
set positional-arguments
set shell := ["bash", "-euo", "pipefail", "-c"]

# The maintainers' bench and release recipes (personal.just.example), when present
import? 'personal.just'

py := if path_exists(justfile_directory() / ".venv/bin/python") == "true" { justfile_directory() / ".venv/bin/python" } else { "python3" }

# The radio for keys, ui and term: a port or a name from radios.toml (`just radio=bench ui`); auto: FERRITE_PORT, the default, the only cable
radio := "auto"

_default:
    @just --list

# Make the in-repo .venv, install the pinned toolchain and cargo-nextest, report the optional parts
setup:
    tools/setup.sh

# Check the installed versions against the pins (installs nothing)
doctor:
    tools/setup.sh --check

# Host tests, then the drivers with every feature and the tools' Python tests (summaries; full log target/just/test.log)
test *args:
    #!/usr/bin/env bash
    set -uo pipefail
    mkdir -p target/just; log=target/just/test.log; : > $log; bad=0
    cargo nextest run "$@" 2>&1 | tee -a $log | grep -E "^ *Summary|FAIL \[|^error" || bad=1
    if [ $# -eq 0 ]; then
        cargo test -p uradio-bus -p bk4829 -p si4732 -p st7789 -p w25q -p keypad -p nmea --all-features >> $log 2>&1 \
            && echo "drivers: ok" || { echo "drivers: FAILED"; grep -E "FAILED|panicked|^error" $log | tail -n 20; bad=1; }
        {{py}} -m unittest -q tools/test_ffw.py >> $log 2>&1 && {{py}} -m unittest discover -q -s tools/tests >> $log 2>&1 \
            && echo "tools: ok" || { echo "tools: FAILED"; bad=1; }
    fi
    [ $bad -eq 0 ] || echo "failed: target/just/test.log"
    exit $bad

# Clippy on the host crates, the radio's build and the reader, warnings as errors
lint:
    cargo clippy -q --all-targets -- -D warnings
    cargo fw-clippy -q -- -D warnings
    cargo fw-reader-clippy -q -- -D warnings
    @echo "lint: ok"

# Build an image: receive, tx, tx-bench, reader (more: tools/mkbtf.sh's usage) -> target/*.btf and .ffw
image kind="tx" *extra:
    mkdir -p target/just
    tools/mkbtf.sh {{ if kind == "receive" { "" } else { kind } }} {{extra}} 2>&1 | tee target/just/image.log | tail -n 4

# Build a radio's image and flash it with its backup (radios.toml; no name: FERRITE_PORT, the default, the only cable)
flash name=radio:
    {{py}} tools/radios.py flash {{name}}

# Your radios (radios.toml), their links, which answer now (--scan: Bluetooth too)
radios *args:
    {{py}} tools/radios.py list "$@"

# Connect to a radio (PORT, else as the tools pick one), read its callsign and version, add it to radios.toml
radio-add name *port:
    {{py}} tools/radios.py add "$@"

# Press keys on the radio over the link: just keys MENU 0 7
keys +seq:
    {{py}} tools/ferrite_link.py {{radio}} keys "$@"

# What the radio's screen shows, as text
ui:
    {{py}} tools/ferrite_link.py {{radio}} ui

# Terminal command lines over the link: just term slot "get squelch"
term +lines:
    {{py}} tools/ferrite_link.py {{radio}} term "$@"

# The UI tests; no arguments: on simulators, in parallel (tools/uitest.py --help for the radio)
uitest *args:
    #!/usr/bin/env bash
    set -uo pipefail
    [ $# -gt 0 ] || set -- --sim auto -j "$( (nproc || sysctl -n hw.ncpu) 2>/dev/null)"
    {{py}} tools/uitest.py "$@" 2>&1 | grep -E "^ui tests:|^results:|FAIL|ERROR|^[^ ].*(error|Error)"

# The build report for a release: target/build-report-KIND.html (with the newest UI results, when there are some)
report kind="tx":
    #!/usr/bin/env bash
    set -uo pipefail
    ui=$(ls -t ../bench-results/*/ui/results.json 2>/dev/null | head -1)
    [ -n "$ui" ] || echo "no UI results (just uitest): the report goes without them"
    {{py}} tools/buildreport.py {{kind}} ${ui:+--ui "$ui"} 2>&1 | tail -n 5

# Regenerate the test plan from the manual's rules (needs ../docs)
testplan *args:
    @if [ ! -d ../docs/manual ]; then echo "skipped: ../docs not found"; else {{py}} tools/testplan.py "$@"; fi

# The manual as HTML (needs ../docs)
manual:
    @if [ ! -d ../docs/manual ]; then echo "skipped: ../docs not found"; else cd .. && {{py}} docs/tools/mkmanual.py; fi

# The decoders' measurements on recordings (ignored tests): the test data is TESTDATA, ../testdata-private or ../testdata
# (tools/testdata.sh); a recording that is missing is skipped
decoder-reports:
    @td=$(tools/testdata.sh); if [ ! -d "$td" ]; then echo "skipped: $td not found (FerriteFW/testdata beside ferrite)"; else cargo nextest run --run-ignored only -E 'binary(report)' --no-capture; fi

# The simulator as wasm for sim-web/
simweb:
    cargo simweb 2>&1 | tail -n 2

# Size change of the last built image against a saved base: `just size-diff save` first, change, rebuild, `just size-diff`
size-diff what="diff":
    @if [ "{{what}}" = save ]; then {{py}} tools/sizemap.py --json target/size-base.json > /dev/null && echo "saved target/size-base.json"; \
    else {{py}} tools/sizemap.py --json target/size-new.json --diff target/size-base.json; fi

# Run a recipe on FERRITE_REMOTE (.env): copies the checkout (and ../docs, ../testdata-private, ../testdata) there first
remote +args:
    tools/remote.sh "$@"
