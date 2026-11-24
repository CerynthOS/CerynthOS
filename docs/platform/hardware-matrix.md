# Platform hardware observations and qualification

November 24, 2026. [Read-only host observations](../benchmarks/person1-hardware-observation.json)
were collected without installation, reboot, scheduler attachment or workload
stress. Device serials, credentials and machine IDs are excluded.

| Environment | Actual observation/test | Candidate OS qualification |
| --- | --- | --- |
| Lenovo 83JE / i7-13700HX | 24 logical CPUs; 16,063,860 KiB RAM reported | Development host only; candidate image not installed |
| Intel display controller | `i915` bound; connected 1920×1080 eDP output | Driver observed on host Linux 7.2.9; no candidate driver/rendering claim |
| NVIDIA RTX 5060 Laptop GPU | `nvidia` bound; driver 615.78.08; 8151 MiB reported | Host query succeeded; no candidate CUDA/driver or VRAM-partition claim |
| x86-64 OVMF/KVM workstation VM | Actual UEFI boot, two vCPUs, virtio disk, standard VGA, Linux 7.3.5 | Base/Plasma services, cpuset/direct I/O, signed installation and A/B recovery tested |
| Pinned Linux 7.1 appliance | Separate minimal kernel and frozen SCX implementation | 123 resource/fault regressions; separate synthetic phase experiment |

The host's working Linux installation cannot establish that a different kernel,
firmware set or desktop image works on that hardware. The VM display is not a
test of the physical Intel/NVIDIA stack. Broader vendor/hardware support remains
unclaimed until the selected image is installed and exercised on an authorized
target. Use the [physical acceptance procedure](release-acceptance.md) to collect
bound recovery evidence and attach actual driver/desktop observations for review.
