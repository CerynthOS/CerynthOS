#!/usr/bin/env python3
"""Synchronized runnable bursts with per-worker completion evidence."""
import argparse
import json
import multiprocessing as mp
from pathlib import Path
import time


def worker(index, gate, counts, stop, cpu_seconds):
    try:
        while not stop.is_set():
            gate.wait(timeout=4)
            if stop.is_set():
                break
            end = time.process_time() + cpu_seconds
            value = index + 1
            while time.process_time() < end:
                value = (value * 1664525 + 1013904223) & 0xffffffff
            counts[index] += 1
            gate.wait(timeout=4)
    except Exception:
        if not stop.is_set():
            stop.set()
            gate.abort()
            raise


def publish(path, counts, bursts, completed):
    record = dict(monotonic_ns=time.monotonic_ns(), worker_completions=list(counts),
                  completed_bursts=bursts, completed=completed)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(record)+'\n')
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--duration', type=float, default=15)
    parser.add_argument('--workers', type=int, default=32)
    parser.add_argument('--cpu-ms', type=float, default=20)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.duration <= 0 or args.workers < 1 or args.workers > 256 or args.cpu_ms <= 0:
        parser.error('positive duration/cpu-ms and 1..256 workers required')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    context = mp.get_context('fork')
    gate = context.Barrier(args.workers+1)
    counts = context.Array('Q', args.workers, lock=False)
    stop = context.Event()
    processes = [context.Process(target=worker, args=(i,gate,counts,stop,args.cpu_ms/1000)) for i in range(args.workers)]
    bursts = 0
    try:
        publish(args.output, counts, bursts, False)
        for process in processes:
            process.start()
        end = time.monotonic()+args.duration
        while time.monotonic() < end and not stop.is_set():
            gate.wait(timeout=4)
            gate.wait(timeout=4)
            bursts += 1
            publish(args.output, counts, bursts, False)
            time.sleep(.1)
        if stop.is_set() or not bursts or any(value != bursts for value in counts):
            raise RuntimeError('not every worker completed every burst')
        publish(args.output, counts, bursts, True)
    finally:
        stop.set()
        gate.abort()
        for process in processes:
            if process.pid:
                process.join(timeout=1)
                if process.is_alive():
                    process.terminate()
                    process.join(timeout=1)
                if process.is_alive():
                    process.kill()
                    process.join()


if __name__ == '__main__':
    main()
