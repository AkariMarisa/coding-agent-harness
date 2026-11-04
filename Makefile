SHELL := /bin/bash
DEV := ./scripts/dev.sh

.PHONY: image build test fmt fmt-check clippy check run clean

image:
	$(DEV) true

build:
	$(DEV) cargo build --workspace

test:
	$(DEV) cargo test --workspace

fmt:
	$(DEV) cargo fmt --all

fmt-check:
	$(DEV) cargo fmt --all -- --check

clippy:
	$(DEV) cargo clippy --workspace --all-targets -- -D warnings

check: fmt-check clippy test

run:
	$(DEV) cargo run -p loom-cli -- $(ARGS)

clean:
	$(DEV) cargo clean
