#!/usr/bin/env python3
"""Emit a CycloneDX inventory with exact package versions and declared licenses."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    database = ROOT / 'artifacts/person1/image-cache/arch~rolling~x86-64~main.metadata.cache/lib/pacman/sync'
    packages = {}
    for path in database.glob('*.db'):
        with tarfile.open(path) as archive:
            for item in archive:
                if not item.name.endswith('/desc'): continue
                fields = {}; key = None
                for line in archive.extractfile(item).read().decode().splitlines():
                    if line.startswith('%'): key = line.strip('%'); fields[key] = []
                    elif line and key: fields[key].append(line)
                packages[(fields['NAME'][0], fields['VERSION'][0])] = fields
    image = json.loads(args.manifest.read_text()); components = []
    for item in image['packages']:
        record = packages[(item['name'], item['version'])]
        components.append({'type': 'library', 'bom-ref': 'arch:' + item['name'] + '@' + item['version'],
            'name': item['name'], 'version': item['version'], 'scope': 'required',
            'hashes': [{'alg': 'SHA-256', 'content': record['SHA256SUM'][0]}],
            'licenses': [{'license': {'name': name}} for name in record.get('LICENSE', [])],
            'properties': [{'name': 'cerynth:origin', 'value': 'Arch snapshot 2026/11/20'},
                           {'name': 'cerynth:license-conclusion', 'value': 'declared upstream; human review required'}]})
    cargo = shutil.which('cargo') or str(Path.home() / '.cargo/bin/cargo')
    metadata = json.loads(subprocess.check_output([cargo, 'metadata', '--locked', '--offline', '--format-version=1'], cwd=ROOT))
    for package in metadata['packages']:
        component = {'type': 'library', 'bom-ref': package['id'], 'name': package['name'], 'version': package['version'],
            'properties': [{'name': 'cerynth:inventory-scope', 'value': 'workspace build graph including build/dev dependencies; binary reachability not asserted'}]}
        if package.get('license'): component['licenses'] = [{'license': {'name': package['license']}}]
        elif package.get('license_file'): component['properties'].append({'name': 'cerynth:license-file', 'value': package['license_file']})
        components.append(component)
    value = {'bomFormat': 'CycloneDX', 'specVersion': '1.6', 'version': 1,
             'metadata': {'component': {'type': 'operating-system', 'name': 'CerynthOS development image', 'version': image['config']['version']},
                          'properties': [{'name': 'cerynth:release-approval', 'value': 'not claimed'},
                              {'name': 'cerynth:bpf-source-license', 'value': 'GPL-2.0 headers retained in schedulers/cerynth-scx and third_party/scx'},
                              {'name': 'cerynth:manifest-sha256', 'value': hashlib.sha256(args.manifest.read_bytes()).hexdigest()}]},
             'components': components}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(value, sort_keys=True, indent=2) + '\n')
    print(json.dumps({'arch_packages': len(image['packages']), 'cargo_build_graph_packages': len(metadata['packages']), 'output': str(args.output)}))


if __name__ == '__main__': main()
