# Platform software qualification follow-up

November 26, 2026. The weekly plan and Person 3 ledger are unchanged. This
follow-up adds independently executable evidence to the existing VM/platform
qualification. It does not represent physical acceptance or another person's
approval.

## Full disk reproducibility

Two independent base-image builds used fresh build roots, the same prepared
runtime overlay, fixed epoch/partition seed, and the same authenticated package
cache. The input manifest was checked again after each build. Both full disks
are **2,216,509,952 bytes**, with SHA256
`7cc46158e7c3d198e4bb803fe1a1e226d1cb9b1e8609a07b473fb5cf83983fd9`.
The streaming comparison found **zero differing bytes**.

The initial comparison found 5,218 differing bytes in glibc's
`/var/cache/ldconfig/aux-cache`. Its contents include build-host inode/device
information. The image recipe removes that disposable accelerator while
retaining `/etc/ld.so.cache`. Repeating both complete builds verified the fix.
The reproduced image then booted through UEFI, started the four expected
services, passed inherited swap/cpuset/direct-I/O enforcement checks, and
preserved a fsynced marker across a VM power cut.

This qualifies the **base disk from a frozen prepared payload on this host and
toolchain**. It does not assert cross-machine reproduction, independent
rebuilding of all upstream packages/Rust executables, or byte-identical desktop
and A/B variants. Both image hashes were retained; the identical second disk
was reclaimed after comparison to conserve development-host space.

Evidence: [comparison](person1-image-reproducibility.json),
[input lock](person1-image-inputs.json),
[initial difference](person1-image-reproducibility-before.json), and
[boot/resource/recovery check](person1-reproduced-image-boot.json).

```sh
python3 tools/image_reproducibility.py --tool-root artifacts/person1/qemu-local/root --output artifacts/person1/rebuild-new
python3 tools/os_image_vm.py --tool-root artifacts/person1/qemu-local/root --image artifacts/person1/rebuild-new/build-1/cerynthos.raw --output artifacts/person1/rebuild-new/boot.json
```

Each destination must be new. Populate the authenticated cache and prepare the
image overlay first as described in the image guide. The tool preserves failed
comparisons for diagnosis and exits unsuccessfully on byte differences or input
drift. It needs space for two disks plus one temporary filesystem tree.

## Tool workflows under contention

Five paired trials compared equal CPU preferences with a Tool/Batch phase policy
inside an actual managed parent and two children. The hard quotas remained
unchanged. Both child workloads voluntarily shared one CPU to create controlled
contention; normal host applications continued running.

| Workload | Verified output | Mean semantic/equal wall ratio | Sample SD |
| --- | --- | ---: | ---: |
| GCC build/link/run | 13 translation units; result checked against an independent calculation | 0.6845 | 0.0193 |
| Repository search/edit/test | Git search and expected patch, 120 passing unit tests | 0.6940 | 0.0403 |
| Artifact archive/extract | All 13 source files survived the round trip | 0.6950 | 0.0633 |

Compilation improved about 32% in this controlled tool workload. The edit/test
measurements were shorter. An earlier exploratory run, before strengthening the
C executable's result checker, found about 29% compilation improvement but a
6% mean archive regression. Those [earlier samples](person1-tool-workloads-exploratory.json)
are retained too. Archive tasks lasted roughly 10 ms and their relative ratios
varied substantially between runs; a reliable archive benefit is not established.
These results do not establish end-to-end
model-driven agent speedups or a broad hardware performance guarantee. No cloud
requests or private repository data were used. Parent cancellation drained all
three resource allocations.

The [raw paired results](person1-tool-workloads.json) retain every measurement,
output checks, compiler/runtime hashes and limitations.

```sh
python3 tools/platform_workload_benchmark.py --binary target/release/cerynth-agentd --repeats 5
```

## SCX bandwidth research

The separate upstream LAVD candidate uses the frozen SCX source revision
`67be0435c454dc8984a11064dc8b9c6476c46b6d`, an independent dependency lock and
unmodified scheduler/library sources. Its copied Cargo workspace excludes
unrelated scheduler/UI members. It is never installed or enabled on the host.
The production Cerynth scheduler baseline is unchanged.

The VM fixture measures actual parent/child CPU usage over three-second windows
and changes quotas between conditions. It tests 20% and 60% parent budgets,
then 10% + 20% child budgets below a 60% parent ceiling, over three repetitions.
An identical Linux-default control distinguishes a real enforcement failure
from a quota/readback-only test. The standalone VM must expose tracefs and its
actual kernel configuration for the upstream loader.

The measured native-SCX result and Linux control are retained separately:

| Requested aggregate CPU fraction | Linux default observed | Upstream LAVD observed |
| --- | ---: | ---: |
| Parent 20% | 0.1996 | 0.1953 |
| Parent 60% | 0.5992 | 0.5904 |
| Children 10% + 20% | 0.2994 | **0.4597** |

The Linux control passed all checks. LAVD exceeded the combined child ceiling
and the 10% child's individual ceiling in all three trials; it is rejected for
production enforcement. Both workers progressed and scheduler termination
returned the VM to Linux. The exact upstream cause is unproven; no speculative
library patch is presented as a repair. A failed native bandwidth check stays
failed. There is no claim of native `cpu.weight` support or arbitrary-SCX
compatibility. Evidence: [upstream experiment](person1-scx-bandwidth.json),
[Linux control](person1-scx-bandwidth-linux.json), and
[candidate build](person1-scx-bandwidth-build.json).

```sh
PATH="$HOME/.cargo/bin:$PATH" python3 tools/build_scx_bandwidth_candidate.py
python3 tools/scx_bandwidth_smoke.py --tool-root artifacts/person1/qemu-local/root --output artifacts/person1/bandwidth-linux-new --linux-control
python3 tools/scx_bandwidth_smoke.py --tool-root artifacts/person1/qemu-local/root --output artifacts/person1/bandwidth-scx-new
```

The final command intentionally fails if sustained child or parent caps fail.
The upstream candidate's option explicitly describes CPU-bandwidth support as
highly experimental. See the [pinned upstream implementation](https://github.com/sched-ext/scx/blob/67be0435c454dc8984a11064dc8b9c6476c46b6d/scheds/rust/scx_lavd/src/main.rs).
