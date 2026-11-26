#!/usr/bin/env python3
"""Bounded owner-invoked CUDA memory-bandwidth fixture; no Torch dependency.

This is a hardware experiment, not enforced GPU isolation or job broker execution.
"""
import argparse
import ctypes
import json
import time


def run(memory_mib, seconds):
    cuda = ctypes.CDLL("libcuda.so.1")
    signatures = {
        "cuInit": [ctypes.c_uint], "cuDeviceGet": [ctypes.POINTER(ctypes.c_int), ctypes.c_int],
        "cuDevicePrimaryCtxRetain": [ctypes.POINTER(ctypes.c_void_p), ctypes.c_int],
        "cuCtxSetCurrent": [ctypes.c_void_p],
        "cuMemGetInfo_v2": [ctypes.POINTER(ctypes.c_size_t), ctypes.POINTER(ctypes.c_size_t)],
        "cuMemAlloc_v2": [ctypes.POINTER(ctypes.c_uint64), ctypes.c_size_t],
        "cuMemsetD8_v2": [ctypes.c_uint64, ctypes.c_ubyte, ctypes.c_size_t],
        "cuCtxSynchronize": [], "cuMemFree_v2": [ctypes.c_uint64],
        "cuDevicePrimaryCtxRelease_v2": [ctypes.c_int],
    }
    for name, parameters in signatures.items():
        getattr(cuda, name).argtypes = parameters
        getattr(cuda, name).restype = ctypes.c_int

    def call(name, *arguments):
        result = getattr(cuda, name)(*arguments)
        if result:
            raise RuntimeError(f"CUDA {name} failed with code {result}")

    call("cuInit", 0)
    device, context, pointer = ctypes.c_int(), ctypes.c_void_p(), ctypes.c_uint64()
    call("cuDeviceGet", ctypes.byref(device), 0)
    call("cuDevicePrimaryCtxRetain", ctypes.byref(context), device)
    try:
        call("cuCtxSetCurrent", context)
        free, total = ctypes.c_size_t(), ctypes.c_size_t()
        call("cuMemGetInfo_v2", ctypes.byref(free), ctypes.byref(total))
        count = memory_mib * 1024 * 1024
        assert free.value >= count + 1024**3, "Need a 1 GiB free desktop/safety margin"
        call("cuMemAlloc_v2", ctypes.byref(pointer), count)
        try:
            call("cuMemsetD8_v2", pointer, 1, count)
            call("cuCtxSynchronize")
            print(json.dumps({"ready": True, "allocated_bytes": count, "duration_limit_seconds": seconds}), flush=True)
            deadline = time.monotonic() + seconds
            iterations = 0
            while time.monotonic() < deadline:
                for _ in range(8):
                    call("cuMemsetD8_v2", pointer, iterations % 255, count)
                call("cuCtxSynchronize")
                iterations += 1
                time.sleep(0.005)
            print(json.dumps({"iterations": iterations, "written_bytes": iterations * 8 * count}), flush=True)
        finally:
            call("cuMemFree_v2", pointer)
    finally:
        call("cuDevicePrimaryCtxRelease_v2", device)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--memory-mib", type=int, default=64)
    parser.add_argument("--seconds", type=int, default=30)
    args = parser.parse_args()
    assert 1 <= args.memory_mib <= 256 and 1 <= args.seconds <= 120
    run(args.memory_mib, args.seconds)
