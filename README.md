# CerynthOS Control Plane

## Overview

This repository contains the initial control plane for **CerynthOS**.

### Completed Modules

  Module                          Status
  ------------------------------- --------
  IPC Wire Protocol               ✅
  Persistent Configuration        ✅
  IPC Server                      ✅
  State Persistence Integration   ✅
  CLI IPC Client                  ✅
  CLI Output Layer                ✅
  Main Wiring                     ✅
  systemd Packaging               ✅
  Unit Tests                      ✅
  Integration Tests               ✅

## Features

-   Versioned IPC protocol
-   Unix domain socket communication
-   Tokio-based daemon
-   Human-friendly CLI
-   Persistent runtime state
-   TOML configuration
-   Real sched_ext scheduler backend
-   CerynthOS cerynth-scx scheduler
-   Live policy engine and telemetry
-   systemd runtime packaging
-   QEMU/KVM development VM
-   Unit and integration tests

## Build

``` bash
cargo build --release --workspace
```

## Fresh Development Setup

```bash
git clone https://github.com/CerynthOS/CerynthOS.git
cd CerynthOS
just bootstrap
just fetch-upstream
just doctor
```

Prepare and run the development VM:

```bash
./scripts/vm/setup-base-vm.sh
./scripts/vm/run-base-vm.sh
```

After the VM is reachable, discover its local root filesystem configuration:

```bash
./scripts/vm/discover-root-filesystem.sh
```

## Run Daemon

``` bash
cargo run -p cerynthd
```

## CLI Commands

Status

``` bash
cargo run -p cerynthctl -- status
```

Get Profile

``` bash
cargo run -p cerynthctl -- profile get
```

Set Profile

``` bash
cargo run -p cerynthctl -- profile set performance
cargo run -p cerynthctl -- profile set balanced
cargo run -p cerynthctl -- profile set interactive
cargo run -p cerynthctl -- profile set background
```

Pause adaptation

``` bash
cargo run -p cerynthctl -- adaptation pause
```

Resume adaptation

``` bash
cargo run -p cerynthctl -- adaptation resume
```

## Testing

All unit tests

``` bash
cargo test
```

Integration tests

``` bash
cargo test -p cerynthd --test control_plane -- --test-threads=1
```

## Code Quality

``` bash
cargo fmt
cargo clippy --all-targets --all-features
```

## Typical Workflow

Terminal 1

``` bash
cargo run -p cerynthd
```

Terminal 2

``` bash
cargo run -p cerynthctl -- status
cargo run -p cerynthctl -- profile set performance
cargo run -p cerynthctl -- profile get
cargo run -p cerynthctl -- adaptation pause
cargo run -p cerynthctl -- adaptation resume
```

## Notes

Integration tests run with `--test-threads=1` because they share the
same Unix socket (`/tmp/cerynthd.sock`) and runtime state.

## Current Development Focus

-   Guarded automatic policy adaptation
-   Scheduler runtime statistics and telemetry
-   Profile-specific scheduling behavior
-   Benchmarking against the Linux default scheduler
-   Packaging and VM reproducibility
