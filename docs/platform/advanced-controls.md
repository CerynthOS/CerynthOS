# Advanced managed resource controls

The candidate `v1alpha1` session budget supports swap, CPU affinity, I/O
preferences, device throughput, and fixed-capacity workspace filesystems.
The schema bundle and protocol fingerprint change with these fields; existing
private contract pins are not silently migrated. Team acceptance is not claimed.

## Owner configuration

Run the supervisor with `CERYNTH_RESOURCE_HIERARCHY=1`. Optional phase scheduling
requires `CERYNTH_PHASE_SCHEDULING=1`. The image includes the system-manager
`user@.service` delegation drop-in for `cpu cpuset io memory pids`.
Staging the runtime alone does not modify the host system manager. On a machine
without the necessary delegated controller, advanced launch fails before the
agent process starts. Reading a requested property is not accepted as proof of
an available kernel controller.

Example addition to `spec.runtime.budget`:

```json
{
  "platform": {
    "memory_swap_max_bytes": 0,
    "allowed_cpus": [0],
    "io_weight": 100,
    "io_devices": [{
      "major": 254,
      "minor": 0,
      "write_bytes_per_second": 1048576
    }]
  }
}
```

Device numbers are examples. Use the actual whole backing block device, not a
partition or tmpfs device. Partition requests are rejected explicitly: Linux's
I/O controller can normalize partition paths to a different whole-device number.
Device-mapper, network filesystems, buffered-write attribution and hardware I/O
schedulers require separate qualification. `io_weight` is a preference, not a
throughput or fairness guarantee. The throughput experiment uses direct I/O.

All settings go through the existing systemd owner. The runtime verifies kernel
swap, requested/effective cpuset, and device throughput settings before launch.
Children inherit omitted bounded parent fields and cannot expand them. The
original CPU/RAM/task/timeout and shared-authority admission checks still apply.

## Workspace capacity

`tools/cerynth-volume` is a trusted owner CLI, not an agent tool. It creates
fixed-size ext4 images mounted through `fuse2fs`, with a private journal and a
bounded helper service. The image and journal must remain outside the sandbox.
The session budget `workspace_max_bytes` bounds the session's workspace-volume
subtree; its own image plus child subtree reservations must fit. The reported
filesystem capacity is smaller than image size because ext4 uses metadata.

```sh
python3 tools/cerynth-volume --directory /absolute/private/volumes init --capacity-bytes 268435456
python3 tools/cerynth-volume --directory /absolute/private/volumes create --id 11111111-1111-4111-8111-111111111111 --image-bytes 33554432 --subtree-bytes-max 134217728
python3 tools/cerynth-volume --directory /absolute/private/volumes mount 11111111-1111-4111-8111-111111111111
```

Set the session workspace to the returned mount path, select required isolation
and cgroups, request `workspace_max_bytes: 134217728`, and configure the owner
supervisor's `CERYNTH_VOLUME_DIRECTORY` to that private authority. Before launch,
the runtime checks ownership, the locked journal, parent reservations, backing
size, actual kernel mount identity, and filesystem capacity. A workspace's own
metadata file grants no authority.

The limit covers this workspace filesystem. The sandbox must deny unrelated
filesystems. Full repository directories can be seeded; seeds containing an
external Git worktree `.git` file are rejected. Automatic worktree conversion
is not provided by this volume CLI. Session termination preserves workspace
artifacts; the owner unmounts and removes volumes separately, descendants first.
FUSE helpers are separate owner services with a 128 MiB memory ceiling,
32-task ceiling and CPU weight 50. Their infrastructure overhead is outside
the agent process subtree's accounting and must be budgeted separately.
Interrupted removal is recoverable. A helper crash requires unmount/remount;
the runtime does not interpret a disconnected FUSE mount as an unmounted volume.

## Phase policy

The opt-in policy adjusts cgroup CPU preferences every 500 ms within the user's
existing ceilings. Interactive work receives greater preference than batch,
idle or approval-wait phases. Active child preference propagates through its
ancestors; terminal sessions retain cleanup preference while their boundary is
still active. Memory pressure reduces background preference. Errors stop the
sampler and appear as `phase_scheduling_status: failed` in diagnostics.

The production resource API continues refusing sched_ext CPU control until its
quota/weight semantics are qualified. The separate `cerynth-scx-phase` target is
a lab experiment, not an installed system service. It substitutes a bounded
slice policy into a hash-checked baseline, reads a private root-owned process
snapshot before attachment, and exits after 60 seconds. No filesystem access is
added to its dispatch path. Its static PID snapshot is suitable only while the
witnessed workers remain alive; it does not provide a PID-reuse or cgroup-budget
guarantee. The frozen scheduler sources remain unchanged.

## Reproduction

```sh
python3 tools/advanced_resources_smoke.py
python3 tools/active_swap_vm.py --tool-root artifacts/person1/qemu-local/root
python3 tools/workspace_quota_smoke.py --tool-root artifacts/person1/qemu-local/root
python3 tools/workspace_session_smoke.py --tool-root artifacts/person1/qemu-local/root
python3 tools/phase_policy_benchmark.py
python3 tools/scx_phase_benchmark.py --tool-root artifacts/person1/qemu-local/root
```

The host advanced-control test deliberately reports unsupported cpuset/I/O
delegation as unsupported. Full qualification runs inside the development OS
VM with `--require-advanced`; no host delegation or scheduler setting is changed.

The active-swap wrapper creates a swap file only inside its disposable VM,
disables guest zswap for the device-I/O check, and removes the swap file after
the fixture. A 100 MiB anonymous workload runs within a 64 MiB resident limit
with a 96 MiB swap budget; the same allocation with a zero swap budget is
contained by a specific cgroup OOM. Memory high equals memory max in this test
to isolate the hard limit from reclaim-throttling latency. Kernel swap-out and
cgroup-specific OOM evidence are retained. No host swap configuration changes.
