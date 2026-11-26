# CerynthOS — Remaining tasks and acceptance handoff

**Audit date:** November 26, 2026  
**Implementation revision audited:** `7d0d32935949feddd47e6d88894986663435a5b5`  
**Development branch:** `feat/person1-platform`  
**Draft integration PR:** [#12](https://github.com/CerynthOS/CerynthOS/pull/12), targeting `feat/person3-agent-runtime`

## Is everything else complete?

**No.** The delivered software and passing tests do not establish completion of
every Person 1/Person 3 roadmap requirement or public-alpha readiness.

The Person 3 acceptance ledger records **105 implemented, 3 partial and 4
pending tasks**. “Implemented” means the scoped behavior and evidence described
in that ledger; it does not mean universal harness support, production
qualification or team approval. All seven partial/pending task IDs appear below.
Person 1 has delivered software/VM evidence, with the remaining acceptance and
capability limits listed separately. No complete numbered Person 1 task ledger
was supplied, so this file does not invent a completed-task count for that track.

The owner deferred cloud configuration until last and said external physical,
signing and review inputs cannot currently be supplied. These items remain
unfinished. The weekly plan and original Person 3 ledger are unchanged.

## 1. Open Person 3 task IDs

| Task | Recorded status | What is unfinished | What is needed to close it |
| --- | --- | --- | --- |
| **P3-010 — Agree interfaces with Persons 1, 2 and 4** | Pending | Actual coordinated interface agreement | Responsible maintainers review the current schema/fingerprint, resource/security/event interfaces and record real decisions with identities and evidence. |
| **P3-085 — Evaluate consented local/cloud routing** | Partial | An authorized live cloud-routing experiment | Configure an owner-selected provider endpoint, dedicated credential environment-variable reference, authorized account/model and explicit session/privacy consent; run the experiment and retain outcomes. Policy tests alone are insufficient. |
| **P3-099 — Stabilize shared APIs and IPC** | Partial | Coordinated compatibility review and stable-release acceptance | Review/adopt the exact candidate contract, supported clients and behavior; resolve review findings and record stabilization acceptance. Hashes, historical round trips and CI drift checks already exist. |
| **P3-105 — Validate the mixed-harness workflow** | Pending | Authenticated local/cloud mixed execution, including Claude Code | Supply authorized harness/account configuration; run and independently verify the actual mixed workflow, cancellations and cleanup. Separate local harness tests do not close this. |
| **P3-106 — Run the five-agent local/cloud scenario** | Pending | The actual reference run | Run the two local Hermes, two cloud coding and one Claude Code sessions with available hardware, authorized models/accounts and separate GPU-tool admission. Retain resource, outcome and cleanup evidence; document deviations from illustrative hardware. |
| **P3-107 — Pin and test supported versions** | Partial | Authenticated cloud feature/version qualification | Exercise the selected pinned provider/harness versions against real accounts; publish supported operations, failures and unsupported features. Offline version inventory and local protocol checks already exist. |
| **P3-112 — Complete release integration and regression reviews** | Pending | Cross-track release review and alpha acceptance | Review the exact integrated candidate and its regression, security, hardware and workload evidence; resolve findings and record actual release acceptance. |

**Cloud setup:** provide endpoint and credential environment-variable names,
not secret values in this file or chat. No live cloud success is claimed by the
current policy-only checks. The four cloud-dependent IDs above remain deferred.

Sources: [Person 3 ledger](docs/roadmap/person3-tasks.md),
[retained release qualification](docs/benchmarks/person3/release-qualification-2026-11-20.json),
[API candidate](docs/contracts/agent-compatibility-candidate.md).

## 2. Person 1 physical acceptance — owner/operator work

These must run on an explicitly authorized disposable target. Existing VM tests
and observations of the development host do not substitute for them.

- [ ] Select the disposable machine, authorized access method and exact target
  disk/serial; establish an installation/recovery and power-test procedure.
- [ ] Install and boot the exact authenticated candidate on that target, retaining
  serial-bound installation/readback evidence and deployed overlay identity.
- [ ] Complete normal-reboot and physical power-cut recovery checkpoints across
  distinct boots, with the persisted marker intact and actual operator attestation.
- [ ] Verify the candidate's real display, GPU/driver and ordinary desktop operation.
- [ ] Exercise representative CPU/RAM/I/O/agent contention and desktop responsiveness
  on the installed target. Retain failures, OOM/fallback/cleanup observations and
  measurements; add supported hardware to the matrix only after qualification.

Existing tools: `tools/platform_hardware_probe.py`,
`tools/platform_physical_recovery.py` and `tools/release_os_image.py`.
Follow the [acceptance procedure](docs/platform/release-acceptance.md).
These tools do not themselves choose a disk or initiate reboots/power cuts.

## 3. Human decisions, integration and public release

These are external acceptance inputs, **not physical-only tasks**. Their tooling
exists, but the actual decisions and production identity have not been supplied.

- [ ] Complete the Person 1/2/4 interface review against the exact current
  contract fingerprint. This overlaps P3-010/P3-099; it is not a second approval.
- [ ] Complete a human dependency/model-license review of the selected release
  inventory and resolve findings. The generated SBOM lists declarations; it is
  not legal/license approval.
- [ ] Select authorized public release publisher and acceptance-authority
  identities and establish their external trust distribution. Disposable
  development test keys do not supply a production identity.
- [ ] Resolve review findings, integrate the stacked branches through the normal
  review process, and build/retest the exact accepted source/configuration.
  PR #12 is a draft; these changes are not represented as merged into `main`.
- [ ] Record genuine interface, license and release decisions with reviewer
  identities and evidence, bound to that candidate and its physical report.
- [ ] Have the authorized authority sign the real acceptance record, then produce
  and verify the public release bundle with the matching authorized publisher.
- [ ] Complete public-alpha release integration/acceptance and publish approved
  artifacts through the project's release process. No public alpha is claimed here.

See the [interface review packet](docs/contracts/person1-interface-review.md)
and [public-release procedure](docs/platform/release-acceptance.md).
No missing approval should be filled using synthetic regression-test records.

## 4. Software capabilities still unsupported or incompletely qualified

These are distinct from missing physical access. Some are explicit boundaries of
the implemented prototype or later research work. A scoped task labelled
“Implemented” does not imply the broader capability below is finished.

| Capability | Current limit / evidence | Work needed before claiming broader support |
| --- | --- | --- |
| **Production SCX CPU quota/weight guarantees** | The resource API refuses SCX CPU control. The separate pinned LAVD experiment allowed about **46%** aggregate CPU for children limited to **10% + 20%**; the identical Linux-default control stayed near 30%. The frozen Cerynth scheduler is unchanged. | Diagnose/fix or select an implementation that passes nested/dynamic quota, weight, starvation, progress and fallback tests. Keep the gate until enforcement is established. This is a failed qualification, not completed native-SCX support. |
| **SCX phase-policy benefit and durable identity** | The static-snapshot slice experiment showed no useful CPU-share benefit. Its identity witness is only valid while the sampled workers remain alive. | Establish an effective measured policy and robust process/cgroup identity lifecycle before production deployment. |
| **Distribution-kernel SCX compatibility** | The frozen scheduler is qualified on the separate Linux 7.1 appliance; the full development image uses Linux 7.3.5. SCX autostart/adaptation remain off in that image. | Qualify the intended scheduler/loader on the exact distribution kernel and recover safely from failures before enabling it there. |
| **Broader image reproducibility** | Two base disks from the same frozen prepared payload/toolchain were byte-identical. Independent upstream-package/Rust builds, other hosts and desktop/A-B bit reproducibility were not qualified. | Rebuild those payloads/variants independently and across selected build environments, retaining input locks and complete comparisons. |
| **Secure Boot** | Development images use `SecureBoot=no`. Artifact Ed25519 signatures are separate from firmware boot-chain verification. | Implement an authorized boot-chain signing/enrollment/update design and validate it on supported firmware before advertising Secure Boot. |
| **General agent performance gains** | Controlled tool/phase experiments exist, including negative/noisy results; they do not prove broad model-driven task improvements. | Run repeated representative end-to-end workloads and baseline comparisons, retaining raw data, tail latency, failures and regressions. |
| **Broader I/O fairness/enforcement** | Direct whole-device writes were tested. I/O weights are preferences. Buffered I/O, device-mapper/network storage and other storage schedulers remain unqualified. | Test the intended storage stacks and distinguish actual enforcement from readback/preferences. |
| **Automatic worktree-to-bounded-volume conversion** | The volume CLI rejects seeds whose `.git` file points to external worktree metadata. FUSE helper overhead is outside the session subtree. | Implement safe conversion/mapping and separately budget infrastructure overhead if those workflows are required. Current full-repository volume seeds remain supported. |
| **Machine-wide resource accounting** | Shared managed-domain reservations do not automatically include unregistered model servers, administrative coordinator grants or bypassing workloads. | Add trustworthy registration/accounting/enforcement for the additional execution paths before claiming complete machine-wide allocation. |
| **General hard GPU isolation** | Monitoring/cooperative reservations and a bounded MPS experiment exist; unsupported generic VRAM limits are not enforced. MIG was unavailable on the test device. AMD physical validation is absent. | Qualify supported hardware/driver enforcement and additional vendors, or retain explicit cooperative/unknown labels. |
| **General KV-cache actuation and isolation** | Session-aware policy/placement are recommendations; they do not implement general KV transfers or placement actuation. LMCache was a bounded experiment. Strict managed-process privacy is specific to supported owned Ollama boundaries. | Integrate/validate engine-native transfer, placement and authorization isolation for each supported engine; assess leakage beyond current process-boundary tests. |
| **Universal harness discovery/resume/remote support** | Native discovery is limited; resume is only advertised when negotiated. A2A and remote execution are supervised prototypes. Claude's authenticated cloud workflow remains unverified. | Implement/test each additional promised operation and protocol/version explicitly. Generic PTY/raw output is not full semantic integration. |
| **General external-effect deduplication** | At-most-once submission applies to the actual brokered tool path. Arbitrary unbrokered shell side effects, exactly-once delivery and automatic rollback are outside the guarantee. | Mediate additional side-effect paths and define provider idempotency/recovery semantics before making broader claims. |

Sources: [platform status](docs/platform/person1-status.md),
[advanced-control boundaries](docs/platform/advanced-controls.md),
[image/SCX/workload evidence](docs/benchmarks/person1-software-followup.md),
[Person 3 ledger](docs/roadmap/person3-tasks.md),
[cache privacy](docs/runtime/cache-privacy.md), and
[tool-effect scope](docs/runtime/tool-effects.md).

## 5. What has already passed

The unfinished items above do not erase these completed checks:

- Existing runtime/session/harness/inference/resource implementation within the
  scopes described in the Person 3 ledger.
- Frozen scheduler baseline: 123 pinned-kernel resource/progress/fault checks.
- Actual base/Plasma UEFI VM boots, resource controls, bounded workspaces, active
  swap containment, signed development installation and A/B recovery tests.
- Two byte-identical complete base disks from a frozen payload; the reproduced
  disk also passed boot/resource/power-loss persistence checks in a VM.
- Thirty final real tool-workload measurements, with exploratory and negative
  results retained rather than converted into general speedup claims.
- 57 Python tests and the full Rust/shell/contract/consent CI on the audited revision.

CI evidence: [full CI](https://github.com/CerynthOS/CerynthOS/actions/runs/45819817430)
and [acceptance/evidence CI](https://github.com/CerynthOS/CerynthOS/actions/runs/45819817536).

## Scope of this handoff

This is an evidence-based Person 1/Person 3 handoff, not certification that the
entire four-person, 44-week master plan is complete. Person 2's complete security
track and Person 4's complete desktop track have not been independently audited
here. A working Guard integration or native observer does not close their full
roadmaps. Reconcile those owners' own task lists before claiming whole-project
completion.
