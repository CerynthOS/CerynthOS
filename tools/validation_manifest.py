#!/usr/bin/env python3
"""Hash a benchmark tree without modifying it or following symlinks."""
import hashlib
import json
import sys
from pathlib import Path


def manifest(root):
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        raise ValueError('baseline must be an existing ordinary directory')
    result = {}
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            raise ValueError(f'baseline contains a symlink: {path}')
        if path.is_file():
            digest = hashlib.sha256()
            with path.open('rb') as source:
                for chunk in iter(lambda: source.read(1024 * 1024), b''):
                    digest.update(chunk)
            result[str(path.relative_to(root))] = digest.hexdigest()
    return result


if __name__ == '__main__':
    print(json.dumps(manifest(sys.argv[1]), indent=2, sort_keys=True))
