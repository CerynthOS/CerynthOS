# Person 1 platform extension review packet

Status: candidate; automated integration evidence is available. This file is
not an approval by Persons 1, 2 or 4, and it does not authorize a public release.

The authoritative candidate identifier is generated in
[`agent-protocol-v1alpha1.manifest.json`](schemas/agent-protocol-v1alpha1.manifest.json).
Review the checked-in request/session schemas alongside the implementation.
The previous private contract pin must fail on mismatch until its owner
explicitly adopts the new fingerprint. The pinned Person 1 platform library's
source and provenance remain unchanged; the session adapter owns the extension.

| Reviewer | Concrete interface to review | Automated evidence |
| --- | --- | --- |
| Person 1 | `PlatformLimits`, systemd properties, controller readback, parent bounds and phase preferences | Booted image swap/cpuset/direct-I/O checks; shared-authority and cleanup tests |
| Person 2 | Required isolation for bounded workspaces, private volume authority, immutable signed update details, child non-escalation | Real sandbox/backing-file denial; forged mount metadata rejection; signed-manifest substitution regression; shared Guard cleanup |
| Person 4 | Optional advanced accounting, workspace-volume capacity/ancestry, configured/running/failed phase status, unchanged unavailable-data semantics | Native observer in booted Plasma/Wayland; KWin D-Bus measurements under managed CPU contention |

Budget fields `platform` and `workspace_max_bytes` are optional and omitted
from legacy serialization when absent. Children inherit omitted restrictions;
explicit expansion is denied. Old clients must not infer support from a field
name alone: negotiated schema identity, owner configuration and actual delegated
controllers all matter. I/O weights are preferences. Device bandwidth names
whole backing block devices; partition requests fail explicitly. Workspace
capacity is tied to an owner-managed fixed-size filesystem, not a file placed
inside an arbitrary workspace.

Diagnostics expose phase sampler failures. A lower-trust phase report can change
preference only within existing owner ceilings; it cannot increase quota, memory,
capabilities or inference reservations. The production resource owner continues
rejecting sched_ext CPU control. The SCX snapshot experiment provides no hard
CPU budget guarantee and is not installed by the image/runtime installers.

Read [advanced controls](../platform/advanced-controls.md),
[image/update behavior](../platform/development-image.md) and the
[current platform status](../platform/person1-status.md) before recording a
decision. Acceptance needs reviewer identity, candidate fingerprint, scope,
decision and evidence. No review entry has been fabricated or sent on anyone's
behalf. The weekly plan and Person 3 task ledger are unchanged.
