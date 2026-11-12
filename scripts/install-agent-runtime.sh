#!/usr/bin/env bash
set -euo pipefail

# Stage only into an explicitly selected root. Does not enable or start services.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
staging_root=""
profile="release"
observer_build=""
while (($#)); do
    case "$1" in
        --root) staging_root="${2:?--root needs a path}"; shift 2 ;;
        --profile) profile="${2:?--profile needs debug or release}"; shift 2 ;;
        --observer-build) observer_build="${2:?--observer-build needs a build directory}"; shift 2 ;;
        *) echo "Usage: $0 --root STAGING_DIRECTORY [--profile debug|release] [--observer-build DIRECTORY]" >&2; exit 2 ;;
    esac
done
if [[ -z "$staging_root" || "$staging_root" == "/" || "$staging_root" != /* ]]; then
    echo "Select an absolute staging directory other than /." >&2
    exit 2
fi
if [[ "$profile" != debug && "$profile" != release ]]; then
    echo "Profile must be debug or release." >&2
    exit 2
fi
if [[ -n "$observer_build" ]]; then
    if [[ ! -x "$observer_build/cerynth-observer" ]]; then
        echo "Build the native observer before staging it." >&2
        exit 1
    fi
    install -D -m 0755 "$observer_build/cerynth-observer" "$staging_root/usr/bin/cerynth-observer"
    install -D -m 0644 "$repo_root/apps/cerynth-observer/io.cerynth.Observer.desktop" \
        "$staging_root/usr/share/applications/io.cerynth.Observer.desktop"
fi
for binary in cerynth-agentd cerynth-worktree-checkout cerynth-harness-bridge cerynth-inferenced cerynth-hardware; do
    source_binary="$repo_root/target/$profile/$binary"
    if [[ ! -x "$source_binary" ]]; then
        echo "Missing $source_binary; build the runtime first." >&2
        exit 1
    fi
    install -D -m 0755 "$source_binary" "$staging_root/usr/lib/cerynth/$binary"
done
for service in cerynth-agentd cerynth-inferenced; do
    install -D -m 0644 "$repo_root/packaging/systemd/user/$service.service" \
        "$staging_root/usr/lib/systemd/user/$service.service"
done
install -D -m 0644 "$repo_root/docs/runtime/agent-runtime.md" \
    "$staging_root/usr/share/doc/cerynth/agent-runtime.md"
echo "Runtime staged at $staging_root. No services were enabled or started."
