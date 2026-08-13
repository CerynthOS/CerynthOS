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

# --- Platform runtime (Person 1) -------------------------------------------

# Install the runtime into a staging root for inspection.
runtime-stage root="/tmp/cerynth-install-test":
    ./scripts/install-dev-runtime.sh --root {{root}} --verbose
    ./scripts/check-runtime-files.sh {{root}}

# Build and install the CerynthOS runtime into the running development VM.
vm-provision:
    ./scripts/vm/provision-cerynth-vm.sh

# Run CerynthOS VM integration smoke tests.
vm-smoke:
    ./scripts/vm/smoke-test.sh

# Return a wedged VM runtime to a known-good state.
vm-recover:
    ./scripts/vm/runtime-recovery.sh

# Provision and smoke test in one step.
vm-verify: vm-provision vm-smoke
