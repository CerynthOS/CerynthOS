# Person 1 platform implementation and acceptance

Updated November 26, 2026. This maps the supplied master plan's Person 1
ownership to concrete code and evidence. The weekly plan and Person 3 task
ledger are unchanged. Automated implementation/VM checks do not constitute
physical hardware acceptance or approval by other team members.

## Delivered and tested

| Responsibility | Implementation and measured scope |
| --- | --- |
| Preserve Linux/SCX baseline | Frozen source/dependency manifest; original scheduler unchanged; four profiles and fault recovery qualified on pinned Linux 7.1 |
| Core supervision/recovery | Endpoint ownership, peer authentication, bounded IPC, healthy profile rollback, atomic state, signal cleanup and fallback tests |
| Process resource hierarchy | One systemd writer; owner/project/parent/child slices; CPU quota, RAM high/max, task limits, cancellation and crash cleanup |
| Advanced controls | Versioned swap, cpuset, I/O preference and whole-device bandwidth API; actual kernel checks before launch; child inheritance/non-escalation |
| Bounded workspace storage | Fixed-size ext4/FUSE volumes, owner/subtree reservations, real ENOSPC, private mount attestation and interrupted cleanup recovery |
| Resource observations | Descendant CPU/RAM/PIDs, PSI, OOM/task events, advanced readback and structured runtime diagnostics |
| Session-aware CPU policy | Owner opt-in phase preferences under Linux default scheduling, bounded ceilings and parent propagation; repeated fixed-work measurements |
| SCX research | Separate hash-based baseline import, preloaded session/process snapshot experiment, lifetime bound, permission/stale-process rejection and five-trial comparisons |
| System responsiveness | Real KDE Plasma/KWin Wayland and Observer boot; D-Bus responsiveness under bounded managed CPU contention |
| OS image and packaging | Pinned Arch package snapshot, base/desktop/A-B mkosi definitions, system/user service integration, wired DHCP and development access |
| Installation | Signed full-image verification, explicit new-file/serial-bound block-target installer, installed-byte readback and boot of the installed image in a VM |
| Upgrade/recovery | Separate root slots plus persistent home/var; authenticated inactive-slot writes, one-shot trial, API/service health acceptance, automatic fallback and persistent manual rollback |
| Signing/inventory | External-key Ed25519 verification, immutable manifest snapshot, tamper/replay rejection, development signatures and CycloneDX declared-license inventory |
| Acceptance execution | Exact-candidate signed review validation, explicit public-channel enforcement and physical recovery checkpoints; synthetic key/record regressions pass |
| Integration/review readiness | Real staged service restart and two-runtime/shared-authority/Guard tests; generated candidate schemas and a review packet |
| Build/regression discipline | Pinned kernel and dependency checks, deterministic kernel appliance/runtime archives, baseline CI workflow, full Rust/Python checks and retained evidence |

Read [advanced controls](advanced-controls.md),
[development image and update instructions](development-image.md),
the [extension measurements](../benchmarks/person1-platform-extensions.md),
and the [interface review packet](../contracts/person1-interface-review.md).
The [acceptance procedure](release-acceptance.md) supplies commands and record
formats for executing the remaining physical/review gates.
The [hardware matrix](hardware-matrix.md) separates observed development-host
drivers from the VM-qualified candidate and unexecuted physical-image tests.
The earlier [foundation report](../benchmarks/person1-platform-validation.md)
remains historical evidence for the baseline and its 123 kernel/SCX checks.

## Actual guarantees

| Control | Qualified behavior | Limit |
| --- | --- | --- |
| CPU quota / RAM / task count | Nested cgroup v2 enforcement under Linux default scheduling | Custom SCX quota/weight semantics remain unqualified |
| CPU preference | Applied/observed cgroup weights; opt-in phase policy | Preference, not dedicated CPU reservation or general agent speedup |
| Swap | Inherited zero limit plus active-swap workload: 100 MiB payload within 64 MiB RAM + 96 MiB swap; zero-swap control hit a contained cgroup OOM | Qualified inside the disposable VM; no physical swap-latency claim |
| Affinity | Requested and effective cpuset plus observed task affinity | Requires delegated cpuset and online permitted CPUs |
| I/O weight | systemd setting and kernel readback | Device fairness depends on storage stack/scheduler |
| I/O throughput | Real direct writes under an enforced whole-device rate | Buffered I/O, layered storage and other hardware need separate qualification |
| Workspace capacity | Actual fixed filesystem capacity and ENOSPC; sandbox hides backing image/journal | Other filesystems depend on the sandbox; external-metadata Git worktree seeds are rejected |
| GPU | Existing shared cooperative accounting integration | No new hardware VRAM partitioning guarantee |
| Desktop | Wayland/Observer and measured VM D-Bus replies under load | No physical input-to-photon, display/GPU-driver or energy claim |
| Images/signatures | Booted UEFI disk images and authenticated development artifacts | Secure Boot and public release trust are separate |

The production resource hierarchy continues refusing SCX CPU control. The
experimental slice policy did not demonstrate a useful CPU-share improvement;
it is not installed as a production service. Its static identity snapshot only
applies while the witnessed workers remain alive. Full-image bit reproducibility
and broad representative workload improvements are not asserted.

## External acceptance deferred

The owner requested completion of available work on November 26 and confirmed
that external target/signing/review inputs cannot be supplied currently. These
gates are deferred without being recorded as successful.

The software and VM evidence cannot complete these actions by itself:

- An authorized disposable physical target for installation/reboots, display/GPU
  driver checks, physical power-loss/recovery and desktop responsiveness tests.
- An authorized public release signing identity and human dependency/license
  review. Development test keys do not establish that identity.
- Coordinated acceptance of the candidate interfaces by the responsible team
  members and a release review. A review packet is not a fabricated approval.

These gates remain explicit. The development images and passing tests do not
certify that every Person 1 public-alpha requirement is complete.
