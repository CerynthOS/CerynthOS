# Kernel build environment

How to build the CerynthOS kernel, what has to be installed to do it, and the
failures worth recognising. The architecture itself is in
[architecture.md](architecture.md).

The kernel is built out of tree: source in `third_party/linux` (pinned by
`kernel/COMMIT`), output in `kernel/build`. Nothing is written into the
source tree, so a spoiled build is fixed with `rm -rf kernel/build`.

## Prerequisites

Host: a Linux box or a WSL2 Ubuntu, 16 GB of RAM to build comfortably, and
the toolchain below. On Debian/Ubuntu:

```bash
sudo apt-get install -y \
    build-essential bc bison flex \
    libssl-dev libelf-dev libdw-dev libncurses-dev dwarves pkg-config \
    zstd cpio kmod
```

`just bootstrap` installs these plus the Rust and VM tooling; on a machine
that only needs to build kernels, the line above is enough.

Why each package is needed:

| Package | Needed for |
| --- | --- |
| `build-essential` | `gcc`, `make`, `ld` |
| `bison`, `flex` | parsing `Kconfig` (`scripts/kconfig/*.l`, `*.y`) |
| `libssl-dev` | module signing, `scripts/sign-file` |
| `libelf-dev` | `objtool`, `resolve_btfids`, module post-processing |
| `libdw-dev` | `scripts/gendwarfksyms`, new in the 7.x tree: `CONFIG_MODVERSIONS` CRCs now come from DWARF, not preprocessed `.cmd` files |
| `libncurses-dev` | `make menuconfig` |
| `dwarves` (`pahole`) | `CONFIG_DEBUG_INFO_BTF`, which is required to load BPF programs with CO-RE relocations — i.e. our scheduler |
| `zstd`, `cpio` | compressed kernel and initramfs |
| `kmod` | `depmod`, `modprobe` when you install modules |

`just doctor` checks all of them, plus the ones the Rust control plane and
the VM need.

### Two settings for a WSL-on-Windows checkout

The repository is developed in WSL against a Windows-backed filesystem, and
git's Windows defaults break a Linux build. Set both in the repo:

```bash
git config core.autocrlf false     # CRLF in a shebang breaks scripts in Linux
git config core.filemode false     # Windows cannot store the executable bit
```

Then make the scripts executable again, because the checkout lost the bits:

```bash
git ls-files -s | grep '^100755 ' | cut -f2 | tr '\n' '\0' | xargs -0 chmod +x
```

With `core.filemode false` git will not report these chmods as changes, and
with `core.autocrlf false` a re-checkout keeps LF endings.

## Commands

```bash
just fetch-upstream     # clone/update third_party/linux and check out kernel/COMMIT
just kernel-config      # scripts/configure-kernel.sh  -> kernel/build/.config
just kernel-build       # scripts/build-kernel.sh       -> kernel/build/arch/x86/boot/bzImage
```

`configure-kernel.sh` copies `kernel/config/base.config` to
`kernel/build/.config`, then forces the options this project depends on and
runs `make olddefconfig` so the config matches the pinned source:

| Option | Why |
| --- | --- |
| `CONFIG_SCHED_CLASS_EXT=y` | `sched_ext`, the class our scheduler plugs into |
| `CONFIG_BPF`, `CONFIG_BPF_SYSCALL` | loading the scheduler's BPF half |
| `CONFIG_BPF_JIT`, `..._JIT_ALWAYS_ON`, `..._JIT_DEFAULT_ON` | the scheduler runs on every wakeup; interpreted BPF is not viable |
| `CONFIG_DEBUG_INFO`, `CONFIG_DEBUG_INFO_BTF` | BTF/CO-RE for the BPF programs, and `pahole` output for tooling |
| `CONFIG_SYSTEM_TRUSTED_KEYS` / `..._REVOCATION_KEYS` off | the distribution config signs modules; the dev kernel does not, and an absent key file otherwise fails the build |

`build-kernel.sh` builds with `O=kernel/build` and
`KCFLAGS=-Werror=implicit-function-declaration` (a missing-protype mistake
should fail the build, not warn), then builds modules. **`O=` must be an
absolute path** — `make -C third_party/linux O=kernel/build` resolves `O=`
relative to the source directory and lands in
`third_party/linux/kernel/build`.

Artifacts:

| File | What it is |
| --- | --- |
| `kernel/build/.config` | the config actually used |
| `kernel/build/vmlinux` | the uncompressed kernel, with symbols and BTF |
| `kernel/build/arch/x86/boot/bzImage` | the bootable image |
| `kernel/build/**/*.ko` | 6 300+ modules from `CONFIG_*=m` |

## Verifying an environment

```bash
just doctor                                   # toolchain + source pin
just kernel-config                            # must end with the option list
grep -E '^CONFIG_(SCHED_CLASS_EXT|BPF_SYSCALL|BPF_JIT|DEBUG_INFO_BTF)=y' \
    kernel/build/.config
just kernel-build                             # ends by printing bzImage
```

A faster smoke test while iterating — it exercises the generated headers,
`objtool`, and the scheduler code without a full build:

```bash
make -C third_party/linux O="$PWD/kernel/build" -j"$(nproc)" kernel/sched/core.o
```

## Troubleshooting

Every entry below was hit for real while bringing this environment up.

| Symptom | Cause | Fix |
| --- | --- | --- |
| `/usr/bin/env: 'bash\r': No such file or directory` | scripts checked out with CRLF | `git config core.autocrlf false`, then re-checkout |
| `./scripts/configure-kernel.sh: Permission denied` | executable bit lost on a Windows-backed checkout | the `chmod +x` line above |
| `fatal error: dwarf.h: No such file or directory` while building `scripts/gendwarfksyms` | `libdw-dev` missing (7.x needs it for `CONFIG_MODVERSIONS`) | `sudo apt-get install libdw-dev` |
| `*** Configuration file ".config" not found!` | `O=` given as a relative path | use an absolute `O=$PWD/kernel/build` |
| `#define 16 16` in `include/generated/asm-offsets.h`, then errors like *type defaults to 'int' in declaration of 'DEFINE_RAW_SPINLOCK'* | build directory left over from a different kernel version | `rm -rf kernel/build`, reconfigure, rebuild |
| `just kernel-config` warns *symbol value 'm' invalid for …* | migrating `base.config` from an older release (`olddefconfig` drops the stale values) | expected, no action |
| `just doctor` reports the kernel source at the wrong commit | `third_party/linux` drifted from `kernel/COMMIT` | `just fetch-upstream` |

The stale-build-directory failure is the nastiest, because the message points
at scheduler source that is perfectly fine. Generated headers
(`asm-offsets.h`, `rq-offsets.h`, `autoconf.h`) encode the *kernel version*
and *config* they were produced from; after switching commits they must be
regenerated. `rm -rf kernel/build` is always safe — it is output, not source.
