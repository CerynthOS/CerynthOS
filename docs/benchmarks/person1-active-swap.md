# Active swap qualification

November 26, 2026. The [raw result](person1-active-swap.json) identifies the
booted image and fixture by SHA256. This is a disposable UEFI/KVM test, not a
physical-hardware acceptance claim.

The runtime applied a 64 MiB RAM limit and 96 MiB swap limit to a managed
100 MiB anonymous allocation. It reached readiness with 44,900,352 bytes charged
to swap and 67,067,904 resident bytes, within the 67,108,864-byte RAM ceiling.
The guest kernel recorded 10,741 additional swap-out pages. Guest zswap was
disabled so this check exercised the swap device rather than only compressed
memory caching.

The same allocation under the same RAM limit with swap set to zero failed
before readiness. The kernel logged `CONSTRAINT_MEMCG` for the exact session
slice. Both workload allocations were released, the core services stayed active,
and the guest swap file was removed. Host swap and scheduler settings were not
modified. The test sets memory high equal to memory max to isolate the hard
capacity boundary; it does not measure reclaim-throttling or physical swap latency.

Reproduce with:

```sh
python3 tools/active_swap_vm.py --tool-root artifacts/person1/qemu-local/root
```
