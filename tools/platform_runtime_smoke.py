#!/usr/bin/env python3
"""Run staged daemon/CLI binaries through persistence and recovery, unprivileged."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/person1/runtime-smoke.json")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="cerynth-p1-runtime-") as directory:
        root = Path(directory)
        stage = root / "stage"
        subprocess.run(["bash", str(ROOT / "scripts/install-dev-runtime.sh"), "--root", str(stage), "--no-enable"], check=True, capture_output=True)
        config = root / "config.toml"
        config.write_text('default_profile = "background"\nadaptation_enabled = true\nadaptation_mode = "shadow"\nscheduler_backend = "mock"\nauto_start = false\n')
        state = root / "state.json"
        socket = root / "control.sock"
        environment = os.environ | {"CERYNTH_CONFIG": str(config), "CERYNTH_STATE": str(state), "CERYNTH_SOCKET": str(socket)}
        def ctl(*arguments):
            result = subprocess.run([str(stage / "usr/bin/cerynthctl"), *arguments], env=environment, text=True, capture_output=True, timeout=10)
            if result.returncode: raise RuntimeError(result.stderr)
            return result.stdout
        def boot():
            process = subprocess.Popen([str(stage / "usr/lib/cerynth/cerynthd")], env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            try:
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    if process.poll() is not None: raise RuntimeError(process.stderr.read().decode())
                    if socket.exists():
                        try:
                            return process, ctl("status")
                        except (OSError, RuntimeError): pass
                    time.sleep(.02)
                raise TimeoutError("staged daemon readiness deadline")
            except BaseException:
                process.kill(); process.wait(timeout=5); raise
        def stop(process):
            process.send_signal(signal.SIGTERM)
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait(timeout=5); raise
            finally: process.stderr.close()
            if socket.exists(): raise RuntimeError("graceful shutdown left an endpoint")
        process = None
        try:
            process, first = boot()
            assert "Background" in first and "Mock" in first, first
            ctl("profile", "set", "interactive")
            stop(process); process = None
            persisted = json.loads(state.read_text())
            assert persisted["profile"] == "interactive", persisted
            process, restored = boot()
            assert "Interactive" in restored, restored
            stop(process); process = None
            state.write_text("interrupted legacy write")
            process, recovered = boot()
            assert "Background" in recovered and "Adaptation      : Disabled" in recovered, recovered
            stop(process); process = None
            report = {"schema_version": 1, "kind": "real-staged-platform-runtime-smoke", "passed": True,
                      "first_boot_uses_configuration": True, "profile_persistence_across_restart": True,
                      "corrupt_state_recovers_with_adaptation_paused": True, "sigterm_removes_socket": True,
                      "staged_executables": True, "backend": "mock", "kernel_scheduler_tested": False,
                      "host_configuration_modified": False}
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))
        finally:
            if process is not None and process.poll() is None: stop(process)


if __name__ == "__main__": main()
