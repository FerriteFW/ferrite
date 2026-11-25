#!/bin/sh
# Prints the test data checkout, by the rule every test and tool follows (ferrite_fuzzkit::testdata_dir is the Rust
# copy): TESTDATA if set (relative to the ferrite checkout when not absolute), else ../testdata-private
# (FerriteFW/testdata-private, maintainers only) if it exists, else ../testdata (FerriteFW/testdata, public; its
# fetch/all.sh adds the third-party sets). The path may not exist: callers check.
cd "$(dirname "$0")/.."
if [ -n "${TESTDATA:-}" ]; then
    case $TESTDATA in /*) echo "$TESTDATA" ;; *) echo "$(pwd)/$TESTDATA" ;; esac
elif [ -d ../testdata-private ]; then
    echo "$(cd ../testdata-private && pwd)"
else
    echo "$(cd .. && pwd)/testdata"
fi
