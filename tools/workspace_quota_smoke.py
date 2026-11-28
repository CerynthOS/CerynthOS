#!/usr/bin/env python3
"""Verify real ENOSPC, persistence, bounded delegation and FUSE crash recovery."""
import argparse
import errno
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import uuid


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tool',default='tools/cerynth-volume')
    parser.add_argument('--tool-root',type=Path)
    parser.add_argument('--output',type=Path,default=Path('artifacts/person1/workspace-quota.json'))
    args=parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='cerynth-quota-') as temporary:
        root=Path(temporary)/'state';identity=str(uuid.uuid4());child=str(uuid.uuid4())
        command=['python3',str(Path(args.tool).resolve()),'--directory',str(root)]
        if args.tool_root:command+=['--tool-root',str(args.tool_root.resolve())]
        def call(*arguments,denied=False):
            result=subprocess.run(command+list(arguments),capture_output=True,text=True,timeout=45)
            if denied:
                assert result.returncode!=0,(arguments,result.stdout);return
            assert result.returncode==0,(arguments,result.stdout,result.stderr)
            return json.loads(result.stdout)['result']
        call('init','--capacity-bytes',str(256*1024**2))
        call('create','--id',identity,'--image-bytes',str(32*1024**2),'--subtree-bytes-max',str(128*1024**2))
        call('create','--id',child,'--parent',identity,'--image-bytes',str(32*1024**2),'--subtree-bytes-max',str(64*1024**2))
        call('create','--id',str(uuid.uuid4()),'--parent',identity,'--image-bytes',str(32*1024**2),'--subtree-bytes-max',str(64*1024**2),denied=True)
        call('create','--id',str(uuid.uuid4()),'--image-bytes',str(32*1024**2),'--subtree-bytes-max',str(192*1024**2),denied=True)
        mounted=False
        try:
            volume=call('mount',identity);mounted=True;workspace=Path(volume['workspace'])
            data=b'bounded-workspace\n'*4096;sentinel=workspace/'sentinel';sentinel.write_bytes(data)
            with sentinel.open('rb') as stream:
                import os
                os.fsync(stream.fileno())
            allocated=0;full=False
            with (workspace/'payload').open('wb',buffering=0) as output:
                for _ in range(64):
                    try:allocated+=output.write(b'x'*(1024**2))
                    except OSError as error:
                        assert error.errno in [errno.ENOSPC,errno.EDQUOT],error;full=True;break
            assert full and allocated<32*1024**2,allocated
            call('unmount',identity);mounted=False
            volume=call('mount',identity);mounted=True;workspace=Path(volume['workspace'])
            assert (workspace/'sentinel').read_bytes()==data
            # Kill only the exact owned volume service derived from this private authority.
            tag=hashlib.sha256(str(root.absolute()).encode()).hexdigest()[:12]
            unit=f'cerynth-volume-{tag}-{identity}.service'
            subprocess.run(['/usr/bin/systemctl','--user','kill','--signal=SIGKILL',unit],check=True,capture_output=True)
            call('unmount',identity);mounted=False
            call('mount',identity);mounted=True
            assert (workspace/'sentinel').read_bytes()==data
            call('unmount',identity);mounted=False
            call('remove',identity,denied=True)
            # Reproduce the durable state after interruption between unlink
            # and journal cleanup. Recovery must not recreate the filesystem.
            journal=json.loads((root/'volumes.json').read_text());journal['volumes'][child]['status']='removing'
            (root/'volumes.json').write_text(json.dumps(journal))
            (root/child/'workspace.ext4').unlink();(root/child/'mount').rmdir();(root/child).rmdir()
            call('remove',child);call('remove',identity)
            assert call('list')==[]
            report={'schema_version':1,'kind':'real-fixed-filesystem-workspace-quota','enospc_observed':True,
                'payload_bytes_before_enospc':allocated,'image_bytes':32*1024**2,'filesystem_capacity_bytes':volume['filesystem_capacity_bytes'],
                'data_persisted_across_unmount_and_helper_crash':True,'parent_and_owner_reservations_bound':True,
                'descendant_first_removal_required':True,'interrupted_removal_recovered':True,'cleanup_complete':True,
                'scope':'workspace filesystem only; a sandbox must deny other host filesystems'}
            args.output.parent.mkdir(parents=True,exist_ok=True);args.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
        finally:
            if mounted:call('unmount',identity)


if __name__=='__main__':main()
