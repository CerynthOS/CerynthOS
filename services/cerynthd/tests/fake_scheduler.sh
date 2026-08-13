#!/bin/bash

echo "fake-cerynth-scx started"
echo "arguments: $*"

trap 'echo "fake-cerynth-scx received SIGTERM"; exit 0' TERM

while true; do
    # When a heartbeat path is provided (e.g. by integration tests), mimic
    # the real scheduler's heartbeat file so the daemon's health check can
    # observe it without needing write access to /run.
    if [ -n "$CERYNTH_HEARTBEAT_PATH" ]; then
        mkdir -p "$(dirname "$CERYNTH_HEARTBEAT_PATH")" 2>/dev/null
        echo "{\"pid\":$$,\"profile\":\"$2\",\"heartbeat\":$(date +%s)}" \
            > "$CERYNTH_HEARTBEAT_PATH" 2>/dev/null
    fi
    sleep 1
done
