# Nonphysical platform qualification — November 27, 2026

This follow-up executes the software checks that were unavailable in the earlier
WSL audit. The current test host is native Linux with KVM, FUSE and an existing
user-systemd delegation. No physical candidate installation, host reboot,
power-cut acceptance or physical firmware enrollment is performed.

## Pinned toolchain

The local inventory matches OpenCode 1.19.5, Hermes 0.23.1, Codex 0.167.0,
Claude Code 2.1.312, Ollama 0.35.0, vLLM 0.25.0, SGLang 0.5.16 and LMCache
0.5.6. Codex 0.167.0 is installed separately from the newer system CLI. Public
CLI provisioning verifies pinned archive and executable hashes and refuses to
overwrite unknown files. Version probes use a private HOME and isolated network
namespace; unavailable pins produce a failed structured report even with Python
optimization enabled. They do not silently pass or substitute installed versions.
The workspace requires the compiler actually used for qualification, Rust 1.99,
rather than retaining the unqualified 1.85 workspace requirement.
The main Rust CI selects 1.99.0 explicitly. A rolling-stable run moved to 1.100
and introduced a new lint after workspace tests passed; the CI pin aligns that
gate with the compiler used for the retained local qualification.

```sh
python3 tools/provision_pinned_cli.py --install codex opencode
python3 tools/qualify_person3_release.py --run-local --pinned-cli-directory artifacts/person3/tools
```

Hermes, Claude, Ollama and the isolated engine environments must also match their
pins. The CLI provisioner above provisions its two explicitly supported public
archives; it does not install or authenticate every harness/engine. The release
qualifier exports the selected paths consistently for its local checks. Its final
exit remains unsuccessful while the seven external acceptance IDs remain open,
even when every local check passes. This is the intended release gate.

## Live cgroups, FUSE and cross-process boundaries

The `cerynthd` owner APIs are exposed as an experimental Rust SDK. The existing
scheduler IPC is unchanged; this SDK is not a newly exposed privileged endpoint.
The integrated managed-session APIs remain owned by `cerynth-agentd`.

Live tests exercise actual CPU throttling, memory attribution, a contained
64 MiB no-swap cgroup OOM, owner-process survival and cleanup. Only one explicitly
delegated disposable user service receives the test cgroups. Existing host
scheduler/user-manager delegation is unchanged. Accounting rejects ordinary text
fixtures as kernel evidence and avoids adding parent usage to the same children
again. Enforcement rejects fake cgroup filesystems and unqualified SCX CPU limits.

FUSE tests create a real bounded filesystem under a private authority. A separate
owner process is killed with SIGKILL; a new authority preserves the healthy helper
and fsynced data. Killing that helper then exercises stale-mount recovery and
remount persistence. Two actual managed processes attempt to read each other's
existing private workspace files and are denied in both directions. Their
backing images/journals are hidden; the bounded child reaches real ENOSPC.

Live testing fixed the incorrect mountinfo field, foreground-helper wait,
user-unit garbage-collection race, permission mask, stale-mount handling,
self-dependent volume removal, journal durability/locking, CPU discovery path
and topology-order alignment. Operation execution now binds idempotency keys to
immutable details and refuses handler execution after a failed durable state
write. This is not a universal exactly-once external-effect guarantee.

```sh
cargo build --locked -p cerynthd --example platform_qualification
cargo build --locked -p cerynth-agentd
python3 tools/qualify_native_platform.py --tool-root artifacts/person1/qemu-local/root
```

The example is explicitly opt-in. Direct resource qualification refuses to run
outside its specifically named delegated fixture service. Helper overhead remains
separate infrastructure, as documented in the advanced-control guide.

## Real Secure Boot VM matrix

Workspace-local sbsigntools 0.9.5 is built from pinned upstream and ccan revisions.
The only local source adjustment marks a diagnostic byte counter used for current
GCC; signature/hash operations and compiler error checking remain intact. The
selected signer/verifier tools do not link GNU-EFI; configure's unused EFI-test
CRT input does not mean those separate upstream EFI test programs were built.
Signed Arch packages supply OVMF editing dependencies without host installation.

```sh
python3 tools/bootstrap_secure_boot_tools.py
python3 tools/secure_boot_vm.py --tool-root artifacts/person1/qemu-local/root
```

Six actual OVMF/KVM cases pass with SecureBoot=1 and SetupMode=0:

| Case | Observed outcome |
| --- | --- |
| Trusted signed UKI | EFI accepts the image; its Linux kernel executes |
| Unsigned UKI | Firmware authentication denies loading; no kernel execution |
| UKI signed by an unauthorized key | Firmware authentication denies loading |
| Modified signed UKI | Firmware authentication denies loading |
| Candidate certificate also present in dbx | Firmware authentication denies loading |
| Invalid update with trusted fallback | Primary is denied; trusted fallback kernel executes |

The firmware reports EFI_ACCESS_DENIED for these rejected loads; the runner also
recognizes EFI_SECURITY_VIOLATION, and records them separately. A timeout or an
arbitrary load error is not counted as a successful security rejection. Independent
`sbverify` checks accept the trusted signature and reject unsigned, unauthorized
and modified images. The signing wrapper verifies before atomic no-clobber
publication, and verification requires an explicitly selected certificate.
`--list` is signature inventory, not trust verification.

The matrix uses disposable keys/private variable-store files only. The fallback
case is a test loader path, not a production A/B Secure Boot update qualification.
There is no production signing identity, physical enrollment or human approval.

## Signed verified-root development image

The optional `--verified-root` profile adds separate dm-verity data/hash
partitions, a signed UKI containing the root hash, and a read-only authenticated
base below an ephemeral writable overlay. It has a 4 GiB root ceiling and a
64 MiB hash partition; it is separate from the desktop/A/B profiles. Automatic
firmware key enrollment is disabled.

```sh
python3 tools/build_os_image.py --tool-root artifacts/person1/qemu-local/root --offline --disk-only --verified-root --secure-boot-key artifacts/person1/secure-boot-vm/throwaway-keys/candidate.key --secure-boot-certificate artifacts/person1/secure-boot-vm/throwaway-keys/candidate.crt
python3 tools/verified_root_vm.py --tool-root artifacts/person1/qemu-local/root
```

The full development image boots in enrolled OVMF with Secure Boot enabled. The
kernel reports a valid read-only verity mapping and the UKI-supplied root hash.
After shutdown, the test changes one root-data byte only in the disposable qcow2
overlay. The next boot reports `device-mapper: verity: ... data block 0 is
corrupted` and cannot reach SSH. The raw backing image remains byte-identical.

Runtime root changes are ephemeral. This does not implement persistent
verified-root A/B migration, confidentiality/encryption, protection against an
already authorized root process, or a production trust-distribution policy.
Default base/desktop/A-B images retain their documented guarantees; selecting a
signing key does not establish human release acceptance.

## Remaining inputs

P3-010, P3-099 and P3-112 still require actual cross-track review/acceptance.
P3-085, P3-105, P3-106 and the cloud part of P3-107 still require explicitly
configured, consented provider/account/model access and live executions.
Production signing/acceptance identities and real license/release decisions
remain unavailable. Physical acceptance remains excluded by the owner's request.
No live cloud request, substituted account, production key or approval is created.
The weekly plan and Person 3 acceptance ledger are unchanged.

## Retained results

Qualified source: `5b0ebed2d436032f962f8a5f6aadf3843d57c17b`. The prepared
payload, signed image build and final native run record this revision with a clean
tracked worktree. Evidence commits add these results without changing that payload.

- [Summary and receipt hashes](../benchmarks/person1-nonphysical-summary-nov27.json):
  389 Rust workspace tests, 65 Python tests and all eight local release checks pass.
- [Native CPU/RAM, FUSE and cross-agent run](../benchmarks/person1-native-platform-nov27.json).
- [Secure Boot matrix](../benchmarks/person1-secure-boot-matrix-nov27.json),
  [toolchain provenance](../benchmarks/person1-secure-boot-toolchain-nov27.json),
  [independent verification](../benchmarks/person1-secure-boot-verification-nov27.json)
  and [signing-wrapper execution](../benchmarks/person1-signing-wrapper-nov27.json).
- [Signed image build](../benchmarks/person1-verified-root-build-nov27.json) and
  [actual boot/corruption rejection](../benchmarks/person1-verified-root-nov27.json).
- [Matching version inventory](../benchmarks/person3/version-inventory-nov27.json)
  and [release gate](../benchmarks/person3/release-qualification-nov27.json).

One concurrent native attempt timed out waiting for pressure admission while the
image build/VM work overlapped. That [failed attempt](../benchmarks/person1-native-pressure-attempt-nov27.json)
is retained. The later serial run passed with the same pressure policy; no pressure
threshold was relaxed. Release qualification remains false because the seven
external acceptance IDs remain open, despite every local check passing.
