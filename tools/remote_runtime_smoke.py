#!/usr/bin/env python3
"""Real pinned-key SSH prototype over loopback, using only generated test keys."""
import argparse
import json
import os
from pathlib import Path
import pwd
import shlex
import signal
import socket
import subprocess
import tempfile
import time
import uuid


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/debug/cerynth-agentd")
    parser.add_argument("--output", default="artifacts/person3/remote-runtime-smoke.json")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    # sshd StrictModes verifies ancestors of AuthorizedKeysFile. /var/tmp is
    # world-writable, so use a temporary private directory beneath the owner home.
    with tempfile.TemporaryDirectory(prefix=".cerynth-remote-", dir=Path.home()) as temporary:
        root = Path(temporary); state = root / "state"; workspace = root / "workspace"; workspace.mkdir()
        env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(root)}
        for name in ["host", "client", "wrong-host"]:
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(root / name)], check=True, env=env)
        (root / "authorized").write_text("restrict " + (root / "client.pub").read_text())
        (root / "authorized").chmod(0o600)
        with socket.socket() as reserved: reserved.bind(("127.0.0.1", 0)); port = reserved.getsockname()[1]
        for name, key in [("known", "host"), ("wrong-known", "wrong-host")]:
            (root / name).write_text(f"[127.0.0.1]:{port} " + (root / (key + ".pub")).read_text())
        config = root / "sshd_config"
        fixed = shlex.join([binary, "--directory", str(state), "relay"])
        config.write_text(f"Port {port}\nListenAddress 127.0.0.1\nHostKey {root/'host'}\n"
            f"AuthorizedKeysFile {root/'authorized'}\nPidFile {root/'sshd.pid'}\n"
            "UsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nAuthenticationMethods publickey\n"
            "StrictModes yes\nPermitUserRC no\nPermitUserEnvironment no\nDisableForwarding yes\nPermitTTY no\n"
            f"PrintMotd no\nPrintLastLog no\nForceCommand {fixed}\n")
        remote_config = {"host": "127.0.0.1", "port": port, "user": pwd.getpwuid(os.getuid()).pw_name,
            "identity_file": str(root / "client"), "known_hosts_file": str(root / "known"),
            "agent_binary": binary, "state_directory": str(state), "remote_consent": True}
        remote_file, request_file = root / "remote.json", root / "request.json"
        remote_file.write_text(json.dumps(remote_config)); remote_file.chmod(0o600)
        agentd = subprocess.Popen([binary, "--directory", str(state), "serve"], env=env,
                                 stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        sshd = None
        try:
            deadline = time.monotonic() + 8
            while not (state / "agentd.sock").exists():
                if agentd.poll() is not None or time.monotonic() > deadline: raise RuntimeError("remote agentd did not start")
                time.sleep(.02)
            sshd = subprocess.Popen(["/usr/bin/sshd", "-D", "-e", "-f", str(config)], env=env,
                stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True)

            def request(operation, file=remote_file):
                request_file.write_text(json.dumps(operation))
                result = subprocess.run([binary, "remote", str(file), str(request_file)],
                    env=env, capture_output=True, text=True, timeout=15)
                if result.returncode: raise RuntimeError(result.stderr.strip())
                return json.loads(result.stdout)

            deadline = time.monotonic() + 8
            while True:
                try: assert request({"type": "list"}) == []; break
                except RuntimeError:
                    if sshd.poll() is not None or time.monotonic() > deadline: raise RuntimeError("pinned SSH relay did not become ready")
                    time.sleep(.05)
            wrong = root / "wrong.json"
            no_consent = root / "no-consent.json"
            no_consent.write_text(json.dumps({**remote_config, "remote_consent": False}))
            try:
                request({"type": "list"}, no_consent)
                raise AssertionError("remote request ignored missing consent")
            except RuntimeError as error:
                assert "explicit consent" in str(error), str(error)
            wrong.write_text(json.dumps({**remote_config, "known_hosts_file": str(root / "wrong-known")}))
            try:
                request({"type": "list"}, wrong)
                raise AssertionError("wrong host key was accepted")
            except RuntimeError as error:
                assert "SSH transport failed" in str(error), str(error)
            session = request({"type": "create", "spec": {"name": "remote-public-fixture", "harness": "generic_pty",
                "workspace": str(workspace), "model": {"name": "none"}, "runtime": {"budget": {"timeout_seconds": 15}}}})
            operation = {"type": "start", "id": session["id"], "operation_id": str(uuid.uuid4()),
                "launch": {"program": "/usr/bin/python3", "args": ["-I", "-c",
                    "from pathlib import Path; Path('remote-proof.txt').write_text('owned'); import time; time.sleep(5)"],
                    "initial_input": None, "pty": False}}
            request(operation)
            replay = request(operation)
            assert replay["replayed"] and replay["action_executed"] is False
            deadline = time.monotonic() + 5
            while not (workspace / "remote-proof.txt").exists():
                if time.monotonic() > deadline: raise TimeoutError("remote managed payload did not execute")
                time.sleep(.02)
            request({"type": "stop", "id": session["id"]})
            deadline = time.monotonic() + 8
            while True:
                stopped = request({"type": "status", "id": session["id"]})
                if stopped["state"] == "cancelled" and not stopped["execution"]["cleanup_pending"]: break
                if time.monotonic() > deadline: raise TimeoutError("remote scope cleanup did not finish")
                time.sleep(.02)
            assert stopped["boundary"] == "bubblewrap_systemd"
            assert (workspace / "remote-proof.txt").read_text() == "owned"
            report = {"kind": "real-ssh-loopback-agent-prototype", "pinned_host_key_verified": True,
                "wrong_host_key_rejected": True, "explicit_generated_identity": True,
                "ambient_ssh_auth_disabled": True, "remote_consent_required": True,
                "managed_payload_executed": True, "operation_replay_did_not_execute": True,
                "remote_cancel_and_cleanup": True, "boundary": stopped["boundary"],
                "physical_second_machine_tested": False, "cloud_requests": 0,
                "network_partition_auto_rollback": False}
            dest = Path(args.output); dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(json.dumps(report, indent=2) + "\n"); print(json.dumps(report, indent=2))
        finally:
            if sshd is not None:
                if sshd.poll() is None: os.killpg(sshd.pid, signal.SIGTERM)
                sshd.wait(timeout=5)
            agentd.terminate()
            try: agentd.wait(timeout=10)
            except subprocess.TimeoutExpired: agentd.kill(); agentd.wait()


if __name__ == "__main__": main()
