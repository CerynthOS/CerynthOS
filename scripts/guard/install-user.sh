#!/usr/bin/env bash
# Install Guard for the current Unix user. Does not start or enable a service.
set -euo pipefail
umask 077
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
cargo build -p cerynth-security -p cerynth-guardd --bins
install -d -m 700 "$HOME/.local/bin" "$HOME/.config/cerynth" "$HOME/.local/state/cerynth/guard" "$HOME/.config/systemd/user"
for binary in cerynth-guardd cerynth-guardctl cerynth-guard-exec cerynth-guard-run; do
    install -m 700 "target/debug/$binary" "$HOME/.local/bin/$binary"
done
install -m 600 packaging/systemd/cerynth-guardd-user.service "$HOME/.config/systemd/user/cerynth-guardd.service"
python3 - <<'PY'
import json, os, pathlib
home = pathlib.Path.home()
config = home / '.config/cerynth/guard.toml'
if config.exists():
    print(f'Preserved existing configuration: {config}')
else:
    runtime = os.environ.get('XDG_RUNTIME_DIR')
    if not runtime:
        raise SystemExit('XDG_RUNTIME_DIR is required in a user login session')
    fields = {'runtime_dir': str(pathlib.Path(runtime) / 'cerynth-guard'),
              'state_dir': str(home / '.local/state/cerynth/guard'),
              'guard_exec': str(home / '.local/bin/cerynth-guard-exec')}
    with config.open('x') as out:
        for key, value in fields.items():
            out.write(f'{key} = {json.dumps(value)}\n')
    print(f'Created {config}')
PY
printf '%s\n' 'Installed. Start explicitly with: systemctl --user daemon-reload && systemctl --user start cerynth-guardd'
