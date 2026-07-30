set shell := ["bash", "-cu"]

default:
    @just --list

bootstrap:
    ./scripts/bootstrap.sh

fetch-upstream:
    ./scripts/fetch-upstream.sh

check:
    cargo check --workspace --all-targets

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
    cargo nextest run --workspace

audit:
    cargo audit
    cargo deny check

ci: fmt lint test

kernel-config:
    ./scripts/configure-kernel.sh

kernel-build:
    ./scripts/build-kernel.sh

scx-build:
    ./scripts/build-scx.sh

doctor:
    ./scripts/doctor.sh
