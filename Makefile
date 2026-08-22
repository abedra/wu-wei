.DEFAULT_GOAL := help

# Pull in local config (API keys, DB path override, ...) if present.
# See .env for the list of supported variables. Never committed (.gitignore).
ifneq (,$(wildcard ./.env))
    include .env
    export
endif

.PHONY: help doctor run build release install-desktop test fmt fmt-check clippy check clean

help:
	@echo "wu-wei — available targets:"
	@echo "  doctor          check that required tools are installed"
	@echo "  run             run the app (cargo run)"
	@echo "  build           debug build"
	@echo "  release         release build"
	@echo "  install-desktop register the app so switchers/docks show the logo (Linux .desktop / macOS .app / Windows Start Menu)"
	@echo "  test            run the test suite"
	@echo "  fmt             apply rustfmt"
	@echo "  fmt-check       check formatting without writing changes"
	@echo "  clippy          run clippy lints (all targets)"
	@echo "  check           fmt-check + clippy + test"
	@echo "  clean           cargo clean"

## Verifies the toolchain this project needs is on PATH:
##   - cargo/rustc (edition 2024 requires rustc >= 1.85)
##   - a C compiler (cc), needed to build bundled SQLite and rustls' ring backend
doctor:
	@command -v cargo >/dev/null 2>&1 && echo "OK  cargo: $$(cargo --version)" \
		|| { echo "MISSING  cargo (install via https://rustup.rs)"; exit 1; }
	@command -v rustc >/dev/null 2>&1 && echo "OK  rustc: $$(rustc --version)" \
		|| { echo "MISSING  rustc (install via https://rustup.rs)"; exit 1; }
	@command -v cc >/dev/null 2>&1 && echo "OK  cc: $$(cc --version | head -1)" \
		|| { echo "MISSING  cc (a C compiler is required to build bundled SQLite/ring)"; exit 1; }
	@if [ -f .env ]; then echo "OK  .env found"; else echo "NOTE  no .env — AI-assisted capture will be disabled"; fi

run:
	cargo run

build:
	cargo build

release:
	cargo build --release

## Registers the app with the host desktop so task switchers/docks resolve a
## real icon instead of a generic placeholder: an XDG `.desktop` entry on
## Linux, an `.app` bundle under ~/Applications on macOS, a Start Menu
## shortcut on Windows. All point at the release binary, not a `cargo run`
## debug artifact `cargo clean` would delete out from under them.
install-desktop: release
	./target/release/wu-wei install-desktop

test:
	cargo test

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

clippy:
	cargo clippy --all-targets

check: fmt-check clippy test

clean:
	cargo clean
