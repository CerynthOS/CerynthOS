#!/usr/bin/env python3
"""Bounded root diagnostic for this Cerynth scheduler only; never controls sched_ext."""
import argparse
import json
import os
from pathlib import Path
import resource
import time


def scheduler_children(daemon_pid, proc_root=Path('/proc')):
    daemon=proc_root/str(daemon_pid)
    if (daemon/'exe').readlink()!=Path('/usr/lib/cerynth/cerynthd'):
        return []
    children=set()
    for entry in (daemon/'task').glob('*/children'):
        children.update(int(value) for value in entry.read_text().split())
    result=[]
    for pid in children:
        proc=proc_root/str(pid)
        try:
            if (proc/'exe').readlink()!=Path('/usr/lib/cerynth/cerynth-scx'):
                continue
            fields=(proc/'stat').read_text().split(')')[-1].split()
            if int(fields[1])==daemon_pid and fields[0] not in ['Z','X','x']:
                result.append((pid,proc,fields))
        except (OSError,ValueError,IndexError):
            continue
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--ready',type=Path,required=True)
    parser.add_argument('--parent-pid',type=int,required=True)
    parser.add_argument('--daemon-pid',type=int,required=True)
    args=parser.parse_args()
    if os.geteuid()!=0:
        parser.error('root is required for this scheduler kernel stack')
    args.output.parent.mkdir(parents=True,exist_ok=True)
    # Stay outside sched_ext so the observer can run if its scheduler blocks.
    # Lowest FIFO priority; bounded CPU per wake; all startup work precedes it.
    resource.setrlimit(resource.RLIMIT_RTTIME,(20000,50000))
    with args.output.open('w',buffering=1) as output:
        os.sched_setscheduler(0,os.SCHED_FIFO,os.sched_param(1))
        args.ready.write_text(json.dumps(dict(pid=os.getpid(),policy='SCHED_FIFO',priority=1,
                                              cpu_soft_limit_us=20000,cpu_hard_limit_us=50000))+'\n')
        deadline=time.monotonic()+900
        progress={}
        last={}
        while time.monotonic()<deadline and os.getppid()==args.parent_pid:
            try:
                try:record=json.loads(Path('/run/cerynth/scheduler.json').read_text())
                except (OSError,ValueError):record={}
                try:stats=json.loads(Path('/run/cerynth/scheduler-stats.json').read_text())
                except (OSError,ValueError):stats={}
                for pid,proc,fields in scheduler_children(args.daemon_pid):
                    current_record=record if record.get('pid')==pid else {}
                    current_stats=stats if stats.get('pid')==pid else {}
                    key=current_stats.get('progress_sequence')
                    previous,progress_at=progress.get(pid,(key,time.monotonic()))
                    if previous!=key:
                        progress_at=time.monotonic()
                    progress[pid]=(key,progress_at)
                    progress_age=time.monotonic()-progress_at
                    if fields[0]!='D' and progress_age<1.5:
                        continue
                    sample=dict(timestamp_ms=int(time.time()*1000),monotonic_ns=time.monotonic_ns(),
                                pid=pid,process_state=fields[0],heartbeat=current_record.get('heartbeat'),
                                progress_sequence=key,progress_age_seconds=progress_age,
                                startup_without_heartbeat=current_record.get('running') is not True,
                                kernel_state=Path('/sys/kernel/sched_ext/state').read_text().strip())
                    for name in ['stack','syscall','wchan']:
                        try:sample[name]=(proc/name).read_text()
                        except OSError as error:sample[name+'_error']=str(error)
                    try:sample['state_after']=(proc/'stat').read_text().split(')')[-1].split()[0]
                    except OSError:sample['state_after']='exited'
                    signature=(fields[0],sample.get('syscall'),key)
                    if signature!=last.get(pid):
                        output.write(json.dumps(sample)+'\n')
                        last[pid]=signature
            except (OSError,ValueError,KeyError,IndexError):
                pass
            time.sleep(.05)


if __name__=='__main__':
    main()
