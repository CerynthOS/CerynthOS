#!/usr/bin/env python3
"""Prefetch a pinned Arch image plan concurrently; pacman still authenticates installation."""
import argparse
import base64
import concurrent.futures
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import time
import urllib.parse
import urllib.request


def main():
    root=Path(__file__).resolve().parents[1]
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--packages',nargs='+',required=True)
    parser.add_argument('--cache',type=Path,default=root/'artifacts/person1/image-package-cache/cache/pacman/pkg')
    parser.add_argument('--database',type=Path,default=root/'artifacts/person1/image-cache/arch~rolling~x86-64~main.metadata.cache/lib/pacman')
    args=parser.parse_args();args.cache.mkdir(parents=True,exist_ok=True)
    records={}
    for database in (args.database/'sync').glob('*.db'):
        with tarfile.open(database) as archive:
            for member in archive:
                if not member.name.endswith('/desc'):continue
                fields={};current=None
                for line in archive.extractfile(member).read().decode().splitlines():
                    if line.startswith('%'):current=line.strip('%');fields[current]=[]
                    elif line and current:fields[current].append(line)
                records[fields['NAME'][0]]=fields
    with tempfile.TemporaryDirectory(prefix='cerynth-image-plan-') as temporary:
        plan=subprocess.check_output(['/usr/bin/pacman','--root',temporary,'--dbpath',str(args.database.resolve()),'--logfile','/dev/null',
            '--config',str(root/'packaging/image/mkosi.sandbox/etc/pacman.conf'),'-Sp','--print-format','%n %l',*args.packages],text=True)
    def fetch(line):
        name,url=line.split(maxsplit=1);record=records[name];filename=record['FILENAME'][0];digest=record['SHA256SUM'][0]
        target=args.cache/filename
        if not target.exists() or hashlib.sha256(target.read_bytes()).hexdigest()!=digest:
            parsed=urllib.parse.urlsplit(url);encoded=urllib.parse.urlunsplit(parsed._replace(path=urllib.parse.quote(urllib.parse.unquote(parsed.path),safe='/-._~')))
            temporary=target.with_name(target.name+'.cerynth-download')
            repo=urllib.parse.urlsplit(url).path.split('/')[-4]
            mirror=f'https://singapore.mirror.pkgbuild.com/{repo}/os/x86_64/{urllib.parse.quote(filename)}'
            candidates=[mirror,encoded]
            for attempt in range(5):
                try:
                    selected=candidates[min(attempt,len(candidates)-1)]
                    with urllib.request.urlopen(selected,timeout=60) as response,temporary.open('wb') as output:
                        while chunk:=response.read(1024**2):output.write(chunk)
                    if hashlib.sha256(temporary.read_bytes()).hexdigest()!=digest:raise RuntimeError('archive checksum mismatch')
                    temporary.replace(target);break
                except Exception:
                    if attempt==4:raise
                    time.sleep(2**attempt)
        if record.get('PGPSIG'):(args.cache/(filename+'.sig')).write_bytes(base64.b64decode(record['PGPSIG'][0],validate=True))
        return {'name':name,'version':record['VERSION'][0],'filename':filename,'sha256':digest,'licenses':record.get('LICENSE',[])}
    with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:packages=list(executor.map(fetch,plan.splitlines()))
    report={'version':1,'packages':packages,'authentication':'pacman Required signatures remain mandatory before installation; this is a prefetch cache'}
    output=root/'artifacts/person1/image-prefetch-manifest.json';output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps({'packages':len(packages),'manifest':str(output)}))


if __name__=='__main__':main()
