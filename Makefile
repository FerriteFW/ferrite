# Thin wrappers around the Cargo aliases (.cargo/config.toml) and tools/mkbtf.sh.
# `make` lists targets. Cargo is the build.

.DEFAULT_GOAL := help

BENCH_FEATURES := ferrite-app/bench,ferrite-radio/bench,ferrite-sim/bench
DRIVERS := -p uradio-bus -p bk4829 -p si4732 -p st7789 -p w25q -p keypad -p nmea

.PHONY: help test test-bench test-drivers clippy fw-clippy check uitest \
	sim simw simweb fw tx tx-bench bringup reader restorer venv clean

help:
	@echo "Host checks:"
	@echo "  make test            cargo test"
	@echo "  make test-bench      host tests with the bench feature"
	@echo "  make test-drivers    chip drivers, all features"
	@echo "  make clippy          host clippy, -D warnings"
	@echo "  make fw-clippy       radio and reader clippy"
	@echo "  make check           test, test-bench, test-drivers, clippy, fw-clippy"
	@echo "  make uitest          UI tests on the simulator"
	@echo "PC radio (extra flags: make simw ARGS=--fresh):"
	@echo "  make sim             every screen to target/sim/*.png"
	@echo "  make simw            the radio in a window"
	@echo "  make simweb          the simulator as wasm"
	@echo "Radio images (tools/mkbtf.sh; clock: make tx-bench CLOCK=clock-240):"
	@echo "  make fw              receive-only image"
	@echo "  make tx              tx/rx build"
	@echo "  make tx-bench        transmit, bench commands, Dry TX on"
	@echo "  make bringup         first image on a new radio"
	@echo "  make reader          read-only backup reader"
	@echo "  make restorer        back to stock without Ferrite"
	@echo "  make venv            Python tools environment"
	@echo "  make clean           cargo clean"

test:
	cargo test

test-bench:
	cargo test --features $(BENCH_FEATURES)

test-drivers:
	cargo test $(DRIVERS) --all-features

clippy:
	cargo clippy --all-targets -- -D warnings
	cargo clippy --all-targets --features $(BENCH_FEATURES) -- -D warnings

fw-clippy:
	cargo fw-clippy -- -D warnings
	cargo fw-clippy --features tx -- -D warnings
	cargo fw-clippy --features tx-dry -- -D warnings
	cargo fw-clippy --features bench -- -D warnings
	cargo fw-reader-clippy -- -D warnings

check: test test-bench test-drivers clippy fw-clippy

uitest:
	python3 tools/uitest.py --sim auto

sim:
	cargo sim $(if $(ARGS),-- $(ARGS))

simw:
	cargo simw $(if $(ARGS),-- $(ARGS))

simweb:
	cargo simweb

# CLOCK is mkbtf.sh's second argument (clock-240, no-vt100, ...). Empty by default.
# The receive-only image has no variant name, so the first argument stays empty.
fw:
	tools/mkbtf.sh "" $(CLOCK)

tx:
	tools/mkbtf.sh tx $(CLOCK)

tx-bench:
	tools/mkbtf.sh tx-bench $(CLOCK)

bringup:
	tools/mkbtf.sh bringup $(CLOCK)

reader:
	tools/mkbtf.sh reader $(CLOCK)

restorer:
	tools/mkbtf.sh restorer $(CLOCK)

venv:
	python3 -m venv .venv && .venv/bin/pip install -r tools/requirements.txt

clean:
	cargo clean
