# Platform extensions: measured development results

November 24, 2026. Detailed samples, artifact digests, source hashes and scope
labels are in [the machine-readable report](person1-platform-extensions.json).
Declared dependencies/licenses are in the [desktop inventory](person1-desktop-sbom.cdx.json).
The earlier [baseline qualification](person1-platform-validation.md) remains
separate. None of these measurements establishes public-release approval.

The full Rust workspace passed 321 tests with three ignored upstream/doc cases;
the Python suite passed 37 tests. Strict Clippy passed for the changed runtime
and experimental target, with narrowly scoped existing baseline-reporter style
exceptions when that frozen module is imported. The upstream dependency tree
was preserved rather than rewritten to satisfy newer style diagnostics.

## Enforced controls and real desktop

The development disk booted through OVMF/UEFI into Linux 7.3.5-arch1-1. The core
daemon, user manager, session supervisor and inference coordinator were active.
The advanced resource test exercised swap inheritance/non-escalation, task
affinity and direct I/O. A 4 MiB write with a 1 MiB/s cap took 4.31 seconds in
the base image; the desktop run took 3.97 seconds. These results concern the
tested whole virtio block device and direct I/O path.

KDE Plasma/KWin Wayland and the native Observer ran in the desktop VM. During
two managed CPU-heavy sessions, 30 KWin D-Bus samples had a 4.47 ms p50 and
6.53 ms p95. Both workloads consumed measured CPU and their resource boundaries
were cleaned up. This is service responsiveness in a VM, not physical display
latency or a universal performance guarantee.

The bounded workspace test reached real ENOSPC, retained a persisted marker
across helper crashes/unmounts, rejected parent/owner reservation overflow and
recovered interrupted removal. Managed-session tests verified the actual mount
and private journal, rejected forged metadata before process creation, and
confirmed that the backing file was hidden inside the sandbox.

## Scheduling experiments

Five paired fixed-work CPU trials compared equal preferences with an opt-in
foreground/tool versus background/batch phase policy. Hard CPU quotas stayed
unchanged. The mean semantic/equal foreground wall-time ratio was 0.7009, with
sample standard deviation 0.0041. Every answer was checked. The fixture pinned
both workloads to one CPU and left normal host activity running; this does not
establish a coding-agent completion speedup.

A separate two-vCPU pinned-kernel VM compared Linux default scheduling,
the unchanged balanced SCX baseline, and the phase snapshot experiment, with
five trials per mode. Foreground CPU shares were approximately 49.9%, 49.0%
and 49.1%, respectively. The experimental slice adjustment did not produce a
useful share improvement in this setup. The result does not justify promotion
or a cgroup CPU guarantee. Both workers made progress, stopping each scheduler
restored Linux scheduling, and unsafe/stale snapshot inputs were rejected.
The raw report retains the individual tick counts and sample deviations.

## Installation, authentication and recovery

The signed full-image installer authenticated an external-key manifest, copied
the actual disk image into a new file, verified the installed bytes and booted
those bytes through UEFI. Negative tests rejected an untrusted key, tampered or
symlinked artifacts and replayed installs. A deterministic substitution attempt
after signature verification confirmed that the parser uses the same immutable
manifest bytes that were authenticated.

The A/B test interrupted a real inactive-root write, rebooted the previous root,
staged an intentionally unbootable trial and observed automatic fallback. A
healthy trial was accepted after API/service checks and persisted as the default
across reboot. An explicit rollback also persisted. Home, var and the stated
configuration allowlist survived. The power-cut experiment exposed an EFI
entry durability issue; final qualification includes stronger post-rename flushes
and retained-entry validation/recovery.

These are guest disk/power operations. No host block device or host scheduler
was modified. Physical installation, real display/GPU drivers, public release
identity, license review and coordinated interface/release acceptance still
need their own evidence. The CycloneDX inventory records 567 installed desktop
packages plus the 347-package Cargo build graph, including build/dev dependencies;
it does not conclude binary reachability or legal approval.
