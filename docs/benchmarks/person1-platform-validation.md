# Platform foundation validation — 2026-11-23

Local platform gates passed with **283 Rust tests, 37 Python tests and 123
real-kernel VM checks**. Three pre-existing Rust tests remain ignored. This
qualifies the tested platform foundation, not every Person 1 roadmap item or
a public-alpha distribution. Full scope and outstanding acceptance are in
[`person1-status.md`](../platform/person1-status.md).

The checked source manifest, binary identities and evidence digests are in
[`person1-platform-validation.json`](person1-platform-validation.json).
Raw output remains under ignored `artifacts/person1/qualification-final/`.

## Environment and outcomes

- Linux 7.1 built from `kernel/COMMIT` using the tracked minimal VM profile;
  GCC 16.2.1 and pahole 1.32. QEMU 11.1.3, KVM, two vCPUs, 512 MiB guest RAM.
- The existing scheduler algorithm and pinned SCX dependency are unchanged.
  Balanced, Interactive, Performance and Background attached and maintained
  progress, fresh heartbeats and correct process/profile identity under load.
- SIGKILL, SIGABRT, injected load failure, deleted/corrupt status files,
  heartbeat freeze, intermittent dispatch errors, complete dispatch stall and
  SysRq-S all returned to Linux scheduling. The freeze test deliberately
  invokes the kernel watchdog. The seven additional fault cases each verified
  a fresh healthy scheduler start afterward.
- Under Linux default scheduling, the guest enforced CPU bandwidth and PID
  denial, contained a child memory OOM, accounted descendant memory, exposed
  pressure, and removed its process-free hierarchy. Weight and swap checks
  are readbacks with the limits documented in the support matrix.
- Actual nested systemd sessions also verified the pinned Person 1 resource
  API, parent/child accounting, bounded admission, leaf-first cancellation and
  SIGKILL recovery. The single systemd writer remains authoritative.
- Staged release daemon/CLI binaries restored saved profiles, used first-boot
  configuration, recovered malformed metadata with adaptation paused, and
  removed the endpoint on SIGTERM. Supervisor rollback/error/framing tests
  use a fake scheduler and are separate from real kernel evidence.
- Guest init and initramfs double builds matched. Development runtime archives
  matched byte for byte; upgrade staging preserved existing configuration.
  Checksums and a 346-package Cargo dependency/license inventory were produced.
- Formatting, shellcheck and Python compilation passed. Clippy completed with
  existing and style warnings; a warning-free platform Clippy gate is not claimed.

## Bounded responsiveness observation

The existing contention tool was rerun against real nested systemd sessions.
Two workers shared one bounded core; the maximum/minimum CPU-usage ratio
was **1.0043**, owner IPC p95 **1.195 ms**, and native Qt offscreen frame-interval
p95 **16.474 ms**. These are single-run synthetic observations, not a Linux
comparison, task-throughput improvement, whole-machine saturation result or
physical KWin/input-latency guarantee.

## Failures retained

The original prebuilt Linux 7.1 image booted and passed its 33 resource checks,
but the frozen scheduler loader rejected its BTF prototype for
`scx_bpf_create_dsq`. A separate loader experiment did not repair that image.
A fresh pinned-source kernel build allowed the unchanged original loader and
scheduler to attach. No dependency update from that experiment is introduced.
The exact original image-generation error has not been proven.

An initial minimal config lacked the I/O-weight interface. Enabling the
appropriate controller resolved that check. Early failed appliance runs and
their console output remain separate from the final successful evidence.

No scheduler was attached to the workstation. Its sched_ext state remained
disabled, and VM processes and test-owned session processes were stopped.
The manual GitHub workflow is provided for reproducibility; no remote CI run
is represented by these local results. Reproduction commands are in
[`kernel/build.md`](../kernel/build.md).
