# Linux kernel architecture

How the kernel is put together, and what each part means for CerynthOS. This
is the Module 1 reference: the five topics below are the ones the platform
work depends on. Build instructions and the verified state of the build
environment are in [build.md](build.md).

Everything here is about the **kernel we actually ship**, Linux `v7.1`,
pinned by commit in `kernel/COMMIT` and checked out at `third_party/linux`.
Where a statement is specific to our tree or our config, it says so.

Related pages:

- [build.md](build.md): prerequisites, `just kernel-config`, `just kernel-build`
- [../scheduler/README.md](../scheduler/README.md): the `sched_ext` scheduler
  that runs on this kernel
- [../contracts/runtime-v1.md](../contracts/runtime-v1.md): what the daemon
  promises the rest of the system

## The five topics and where they live

| Topic | Code in `third_party/linux` |
| --- | --- |
| Kernel space vs user space | `arch/x86/entry/`, `mm/`, `include/linux/uaccess.h` |
| System calls and interrupts | `arch/x86/entry/syscall_64.c`, `arch/x86/kernel/irq.c`, `kernel/softirq.c` |
| Process lifecycle and context switching | `kernel/fork.c`, `kernel/exit.c`, `kernel/sched/core.c`, `arch/x86/kernel/process_64.c` |
| Kernel modules and device drivers | `kernel/module/`, `drivers/`, `scripts/gendwarfksyms/` |
| Source structure | the tree itself; see the map at the end |

---

## 1. Kernel space vs user space

The CPU has privilege levels (rings). The kernel runs at ring 0 and can do
anything; user space runs at ring 3 and can only touch memory its own page
tables permit. The split is enforced by hardware, not by convention, and it
is the reason the two halves of CerynthOS communicate through well-defined
interfaces instead of calling into each other.

What each half of this repository does:

| Component | Space | Notes |
| --- | --- | --- |
| `services/cerynthd` (the daemon) | user | Unix socket server, policy engine, telemetry |
| `cli/cerynthctl` | user | IPC client only; never touches the kernel |
| `schedulers/cerynth-scx` (the scheduler process) | user | the dispatch loop and policy decisions |
| its BPF programs (`main.bpf.c`) | kernel | loaded into the kernel, run on kernel paths |
| `crates/cerynth-policy` | user | pure functions: tasks in, scheduling decisions out |
| `sched_ext` core (`kernel/sched/ext.c`) | kernel | calls into the BPF half, enforces the watchdog |

`sched_ext` is the interesting case because it deliberately breaks the neat
picture: scheduling decisions that used to be made in kernel space are made
by a **user-space process** whose code runs in the kernel as BPF. That means
a decision can be as slow as a syscall or a page fault, and the machine
stalls while it happens. Two consequences are baked into the design:

- The dispatch loop must never take a page fault or block. The vendored
  `scx_rustland_core` installs a seccomp filter that makes later `mmap()`
  calls fail, on purpose. See "Starting again" in
  [../scheduler/README.md](../scheduler/README.md).
- Status and statistics are written to `/run/cerynth/` by a separate
  reporter process, not by the loop that is on the critical path.

Boundary crossings cost something. A syscall is a mode switch plus a stack
switch (a few hundred nanoseconds on current hardware). A page fault is far
worse, and an I/O that sleeps is worse again. Design rule for anything that
runs on a hot path here: do the work in user space, then hand the result over
in one syscall — which is exactly what the ring buffers in `main.bpf.c` and
the one-shot syscalls in the dispatch loop do.

## 2. System calls and interrupts

### System calls

A syscall is a controlled transition from user to kernel space. On x86-64
Linux the user side puts the syscall number in `rax`, arguments in `rdi`,
`rsi`, `rdx`, `r10`, `r8`, `r9`, and executes `syscall`; the CPU switches to
ring 0 and jumps to `entry_SYSCALL_64`, which saves registers into a
`struct pt_regs`, dispatches through `sys_call_table`, and returns with
`sysretq`. The number is validated against `NR_syscalls` before the table
lookup, so a bad number returns `-ENOSYS` instead of crashing.

Conventions that matter when reading kernel code:

- Errors are negative `-Exxx` values returned in `rax`; the C library turns
  them into `errno`.
- Anything that can sleep (allocate, wait, take a mutex) may only be called
  from process context, never from an interrupt.
- Data copied to/from the caller's address space goes through
  `copy_to_user()`/`copy_from_user()` (`include/linux/uaccess.h`), which
  validates the pointer. Dereferencing a user pointer directly is a bug, and
  with `CONFIG_HARDENED_USERCOPY` a fatal one.
- Some calls never enter the kernel: the vDSO maps `clock_gettime()` and
  friends read-only into user space.

Which syscalls this project leans on: `epoll`/`poll` in the daemon's
event loop, `clock_gettime` (through the vDSO) for scheduler timestamps,
`bpf()` for loading the scheduler, `sched_setattr()`/`sched_setaffinity()`
for the runtime, and plain file writes for `/run/cerynth/*.json`.

### Interrupts

Interrupts come from three sources, and the distinction matters when
debugging a stall:

| Kind | Source | Example |
| --- | --- | --- |
| Hardware IRQ | a device | the virtio NIC in the development VM |
| Exception | the CPU reacting to an instruction | `#PF` page fault, `#GP`, `int3` |
| Software interrupt/trap | an instruction or the kernel itself | `int3` breakpoints, IPIs, `syscall` |

Hardware IRQs arrive with interrupts disabled on that CPU and must finish
fast. Real work is deferred to softirqs, tasklets, or workqueues, which run
later in a context where sleeping rules still apply. The timer is what makes
scheduling preemptive: with our config, `CONFIG_HZ=1000` means a tick every
1 ms on a CPU that is doing work, and the tick is where
`scheduler_tick()` runs and a task that exhausted its slice is preempted
(`CONFIG_PREEMPT=y` in our config, so the kernel itself is preemptible too).

The `sched_ext` hooks all sit on these paths:

| Kernel path | What happens with `sched_ext` loaded |
| --- | --- |
| wakeup (`try_to_wake_up()`) | `ops.select_cpu()` picks an idle CPU, `ops.enqueue()` is called |
| tick / slice expiry | `ops.running()`/`ops.stopping()` report the task's last run |
| CPU hotplug | the kernel asks the scheduler to restart (exit code 75) |
| `SysRq-S` | `sched_ext` is disabled underneath the scheduler |

A scheduler that stops reacting is not allowed to wedge the machine: the
kernel watchdog ejects it once a runnable task has waited longer than
`timeout_ms` (5 s here). That is the safety net described in
[../scheduler/README.md](../scheduler/README.md).

## 3. Process lifecycle and context switching

### Lifecycle

A task is a `struct task_struct` (`include/linux/sched.h`), with its kernel
stack, its address space (`struct mm_struct`), and its scheduler state. The
kernel itself is a set of kernel threads; user processes are tasks with an
`mm`.

```
fork()/clone()  ->  task_struct allocated, copied from parent (COW pages)
execve()        ->  the same task replaces its address space and images
exit()          ->  zombie until the parent reaps it; task_struct freed after
```

States you will see in `ps`/`/proc`: `R` runnable or running, `S`
interruptible sleep, `D` uninterruptible sleep (usually I/O, and usually a
sign of trouble if it persists), `T` stopped, `Z` zombie.

### Context switching

A running task gives up the CPU on its own (it blocks, sleeps, yields,
exits) or is taken off it (its slice expired, a higher-priority task woke).
Either way the path ends in `__schedule()` in `kernel/sched/core.c`:

1. pick the next task — with `CONFIG_SCHED_CLASS_EXT=y`, our scheduler's
   `ops.dispatch()` is what fills the runqueue first, and the built-in
   classes are the fallback;
2. `context_switch()` → `switch_to()` saves callee-saved registers and the
   stack pointer, and loads the next task's;
3. if the next task is in a different address space, switch CR3 and load the
   new task's page tables — the expensive part, since it invalidates cached
   translations the old task was using.

A switch is on the order of a microsecond plus TLB effects. That number is
why the scheduler profiles exist: `performance` runs 10 ms slices to switch
less, `interactive` runs 0.5–2 ms slices to switch more often but keep
latency low. For the `sched_ext` per-task fields the policy reads —
`exec_runtime`, `weight`, `stop_ts` — see
[../scheduler/README.md](../scheduler/README.md).

One detail worth knowing, because it was a real bug here: a slice of `0` does
**not** mean "use the default". The kernel treats it as "keep the remainder",
and a task with nothing left gets 1 ns. `policy::slice_for()` therefore
always returns a non-zero slice.

The BPF half needs to read kernel structures (`task_struct`, `rq`), and it
does so by field offset. Those offsets are generated during the build
(`include/generated/rq-offsets.h`, `asm-offsets.h`) and compiled into the BPF
program, which is why the scheduler must be rebuilt against the kernel it
will run on. This is also why a stale build directory is dangerous — see the
troubleshooting table in [build.md](build.md).

## 4. Kernel modules and device drivers

Code can be built in (`=y`) or as a loadable module (`=m`). Our config has
both, heavily:

| | count in `kernel/build/.config` |
| --- | --- |
| built in (`=y`) | ~3 250 symbols |
| modules (`=m`) | ~6 330 symbols |
| `CONFIG_MODULES` | `y` |
| `CONFIG_MODVERSIONS` | `y` |
| `CONFIG_MODULE_SIG` | `y` |

A module is an ELF object with `init_module()`/`cleanup_module()` entry
points; it can only use symbols the kernel exports (`EXPORT_SYMBOL`), and it
is version-checked against the running kernel through `vermagic`, which
includes the kernel release and, with `CONFIG_MODVERSIONS`, a per-symbol CRC.

In the 7.x tree that CRC comes from `scripts/gendwarfksyms/`, which reads
DWARF (`libdw`) instead of the old preprocessed `.cmd` files. That is the
tool that failed to link on this machine until `libdw-dev` was installed;
worth remembering, because it is a new dependency compared to 6.x trees.

Device drivers are ordinary modules or built-ins under `drivers/`, bound to
a device through the bus (`pci`, `platform`, `virtio`, `usb`). The
development VM is QEMU/KVM, so the drivers that matter today are virtio
(`CONFIG_VIRTIO=y`): virtio-blk for the root disk, virtio-net for the
network, and the serial console. There is no CerynthOS-specific driver yet;
when one is needed it belongs in `drivers/` in the kernel tree, or in a
standalone out-of-tree module during development.

Runtime tools: `insmod`/`modprobe` load modules into the running kernel,
`lsmod` lists them, and modules must be installed to
`/lib/modules/$(uname -r)/` for `modprobe` to find them —
`make modules_install` in the build directory.

Two things here are **not** modules, which trips people up:

- `sched_ext` is built in. `CONFIG_SCHED_CLASS_EXT` is a `bool`, and the
  scheduler class cannot be a module. The scheduler you can load and unload
  is a BPF program, and loading it is a syscall, not `modprobe`.
- BPF programs are not modules either. They are validated bytecode loaded
  with `bpf(BPF_PROG_LOAD, ...)`.

## 5. Linux kernel source structure

The parts of the tree this project touches, in reading order:

| Path | What it is |
| --- | --- |
| `init/main.c` | `start_kernel()`: boot sequence, ends by running `init` |
| `kernel/fork.c`, `kernel/exit.c` | task creation and teardown |
| `kernel/sched/core.c` | the core scheduler: runqueues, `__schedule()`, class dispatch |
| `kernel/sched/fair.c` | CFS/EEVDF, the default class, and the fallback when `sched_ext` is off |
| `kernel/sched/ext.c`, `ext_idle.c` | `sched_ext` core: the BPF-facing class and its idle-CPU helper |
| `include/linux/sched.h` | `struct task_struct` |
| `arch/x86/entry/` | syscall and interrupt entry |
| `arch/x86/boot/` | the compressed boot image (`bzImage`) |
| `mm/` | memory management: page tables, page faults, allocation |
| `include/uapi/` | headers that user space is allowed to include |
| `scripts/` | build machinery: `kconfig/`, `mod/`, `gendwarfksyms/`, `objtool/` |
| `drivers/`, `fs/`, `net/`, `security/` | subsystems we enable but rarely modify |
| `tools/` | user-space tools, including `bpf/` and `perf/` |

What is ours rather than upstream:

| Path | Purpose |
| --- | --- |
| `third_party/linux/` | the kernel source, ignored by git, pinned by `kernel/COMMIT` |
| `kernel/COMMIT`, `kernel/VERSION` | the exact commit and its release (`v7.1`) |
| `kernel/config/base.config` | the config the kernel is built from |
| `kernel/build/` | out-of-tree build directory (`.config`, objects, `bzImage`) |
| `scripts/configure-kernel.sh`, `build-kernel.sh` | the two commands that do the work |

Two rules that keep this clean:

1. **The source tree is never edited in place.** It is a detached checkout of
   a specific commit; local edits would be lost on the next
   `scripts/fetch-upstream.sh`, and would make "which kernel are we running"
   unanswerable. Changes go in `kernel/config/base.config` (options) or as
   patches applied during the build (not set up yet).
2. **Build output never goes into the source tree.** `O=kernel/build` keeps
   objects out of `third_party/linux`, so the checkout stays pristine and a
   stale tree can be deleted with one `rm -rf`.

Upgrading the kernel is therefore three steps: write the new commit into
`kernel/COMMIT` and its release into `kernel/VERSION`, run
`just fetch-upstream`, then delete `kernel/build` before rebuilding. Skipping
the last step is how the build ends up mixing generated headers from two
kernel versions.

---

## What "working" means for this module

The build environment is done when all of the following hold on a clean
machine:

1. `just doctor` reports no `[FAIL]` for the kernel toolchain;
2. `third_party/linux` is at the commit in `kernel/COMMIT`;
3. `just kernel-config` produces `kernel/build/.config` with
   `CONFIG_SCHED_CLASS_EXT=y`, `CONFIG_BPF_SYSCALL=y`, `CONFIG_BPF_JIT=y`,
   `CONFIG_DEBUG_INFO_BTF=y`;
4. `just kernel-build` produces `kernel/build/arch/x86/boot/bzImage`.

Steps 1–4, the exact package list, and the failures seen while getting there
are in [build.md](build.md).
