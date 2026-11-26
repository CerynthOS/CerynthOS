#!/usr/bin/env python3
"""Bounded, offline-model vLLM/SGLang cache experiments on public synthetic text.

Owns only the backend it launches. It never downloads models, imports host
credentials, changes a running backend, or executes model-generated code.
"""
import argparse
import json
import os
from pathlib import Path
import random
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid


def gpu_memory():
    result = subprocess.run(["nvidia-smi", "--query-gpu=memory.total,memory.used",
        "--format=csv,noheader,nounits"], capture_output=True, text=True, timeout=5, check=True)
    return [[int(n.strip()) for n in line.split(",")] for line in result.stdout.splitlines()]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--engine", choices=["vllm", "sglang"], required=True)
    parser.add_argument("--python", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--served-model", default="Qwen/Qwen3-0.6B")
    parser.add_argument("--cache", choices=["enabled", "disabled"], default="enabled")
    parser.add_argument("--native-sampler", action="store_true", help="Use the engine's PyTorch sampler instead of FlashInfer JIT sampling")
    parser.add_argument("--lmcache", action="store_true", help="Explicit bounded CPU offload experiment, vLLM only; no remote storage or telemetry")
    parser.add_argument("--coordinator-binary", help="Also verify per-grant cache isolation through this built coordinator (vLLM only)")
    parser.add_argument("--trials", type=int, default=3)
    parser.add_argument("--startup-seconds", type=int, default=240)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    assert not args.lmcache or args.engine == "vllm"
    assert not args.coordinator_binary or (args.engine == "vllm" and args.cache == "enabled" and not args.lmcache)
    assert 1 <= args.trials <= 10 and 1 <= args.startup_seconds <= 600
    # Preserve the venv executable symlink: resolving it would select the base
    # interpreter and lose the environment's installed engine packages.
    python, model, dest = Path(args.python).absolute(), Path(args.model).resolve(), Path(args.output).resolve()
    assert python.is_file() and (model / "model.safetensors").is_file()
    mem = {k: int(v.split()[0]) * 1024 for k, v in
           (line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines()) if v.split() and v.split()[0].isdigit()}
    assert mem["MemAvailable"] >= 6 * 1024**3, "Need 6 GiB available RAM for the bounded backend experiment"
    memory_max = min(8 * 1024**3, mem["MemAvailable"] - 2 * 1024**3)
    before = gpu_memory()
    assert len(before) == 1 and before[0][0] - before[0][1] >= 5 * 1024, "Need one GPU and 5 GiB free VRAM"
    dest.parent.mkdir(parents=True, exist_ok=True)
    unit = f"cerynth-research-{uuid.uuid4()}.scope"
    key = uuid.uuid4().hex + uuid.uuid4().hex
    with tempfile.TemporaryDirectory(prefix="cerynth-engine-") as temporary:
        root = Path(temporary); home = root / "home"; home.mkdir()
        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0)); port = reserved.getsockname()[1]
        endpoint = f"http://127.0.0.1:{port}"
        if args.engine == "vllm":
            command = [str(python), "-m", "vllm.entrypoints.openai.api_server", "--model", str(model),
                "--served-model-name", args.served_model, "--host", "127.0.0.1", "--port", str(port),
                "--api-key", key, "--dtype", "half", "--max-model-len", "1024", "--max-num-seqs", "2",
                "--gpu-memory-utilization", "0.45", "--kv-cache-memory-bytes", str(256 * 1024**2),
                "--enforce-eager", "--seed", "0",
                "--enable-prompt-tokens-details",
                "--enable-prefix-caching" if args.cache == "enabled" else "--no-enable-prefix-caching"]
            if args.lmcache:
                configuration = root / "lmcache.yaml"
                configuration.write_text("chunk_size: 256\nlocal_cpu: true\nmax_local_cpu_size: 0.25\nlocal_disk: null\nremote_url: null\nenable_async_loading: false\n")
                command += ["--kv-transfer-config", json.dumps({"kv_connector": "LMCacheConnectorV1Dynamic",
                    "kv_role": "kv_both", "kv_connector_module_path": "lmcache.integration.vllm.lmcache_connector_v1"})]
        else:
            command = [str(python), "-m", "sglang.launch_server", "--model-path", str(model),
                "--served-model-name", args.served_model, "--host", "127.0.0.1", "--port", str(port),
                "--api-key", key, "--dtype", "float16", "--context-length", "1024",
                "--mem-fraction-static", "0.45", "--max-total-tokens", "2048", "--max-running-requests", "2",
                "--disable-cuda-graph", "--attention-backend", "triton", "--enable-cache-report", "--random-seed", "0"]
            if args.cache == "disabled": command += ["--disable-radix-cache"]
            if args.native_sampler: command += ["--sampling-backend", "pytorch"]
        uid = os.getuid()
        environment = {"PATH": f"{python.parent}:/usr/local/bin:/usr/bin:/bin", "HOME": str(home),
            "HF_HOME": str(home / ".huggingface"), "HF_HUB_OFFLINE": "1", "HF_HUB_DISABLE_TELEMETRY": "1",
            "DO_NOT_TRACK": "1", "VLLM_NO_USAGE_STATS": "1", "PYTHONUNBUFFERED": "1",
            "LMCACHE_TRACK_USAGE": "false",
            "OMP_NUM_THREADS": "2", "MKL_NUM_THREADS": "2", "TOKENIZERS_PARALLELISM": "false",
            "XDG_RUNTIME_DIR": f"/run/user/{uid}", "DBUS_SESSION_BUS_ADDRESS": f"unix:path=/run/user/{uid}/bus"}
        # A fixed shell guards only parent identity; command arguments are positional.
        wrapper = ["systemd-run", "--user", "--scope", "--quiet", "--collect", f"--unit={unit}",
            f"--property=MemoryHigh={memory_max * 9 // 10}", f"--property=MemoryMax={memory_max}", "--property=TasksMax=512",
            "--property=CPUWeight=50", "--property=KillMode=control-group", "--", "/usr/bin/setpriv",
            "--pdeathsig", "KILL", "--no-new-privs", "/usr/bin/sh", "-c",
            '[ "$PPID" = "$1" ] || exit 125; shift; exec "$@"', "cerynth-research-parent", str(os.getpid()), *command]
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *_args, **_kwargs): return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
        report = {"kind": "bounded-engine-cache-experiment", "engine": args.engine,
            "cache": args.cache, "model": args.served_model, "context_limit": 1024,
            "gpu_memory_before_mib": before, "cloud_inference_requests": 0,
            "host_credentials_inherited": False, "sandbox": False,
            "memory_max_bytes": memory_max, "host_memory_available_before": mem["MemAvailable"], "trials": [], "success": False}
        versions = subprocess.run([str(python), "-c", "import importlib.metadata,json,sys,sysconfig; print(json.dumps({'python':sys.version.split()[0],'site_packages':sysconfig.get_path('purelib'),'packages':{name:importlib.metadata.version(name) for name in sys.argv[1:]}}))",
            args.engine, "torch", "flashinfer-python"], env=environment, capture_output=True, text=True, check=True, timeout=10)
        report["versions"] = json.loads(versions.stdout)
        cuda_home = Path(report["versions"].pop("site_packages")) / "nvidia" / "cu13"
        if (cuda_home / "bin" / "nvcc").is_file():
            environment["CUDA_HOME"] = str(cuda_home)
            environment["PATH"] = str(cuda_home / "bin") + ":" + environment["PATH"]
            environment["LD_LIBRARY_PATH"] = str(cuda_home / "lib")
            environment["LIBRARY_PATH"] = str(cuda_home / "lib")
            # CUDA's Python runtime wheel supplies the versioned SONAME but no
            # development linker alias. Keep that alias in this private fixture,
            # without changing the installed package or system toolkit.
            runtime_library = cuda_home / "lib" / "libcudart.so.13"
            if runtime_library.is_file() and not (cuda_home / "lib" / "libcudart.so").exists():
                link_directory = root / "cuda-link"; link_directory.mkdir()
                (link_directory / "libcudart.so").symlink_to(runtime_library)
                environment["LIBRARY_PATH"] = str(link_directory) + ":" + environment["LIBRARY_PATH"]
        environment["MAX_JOBS"] = "1"
        if args.engine == "vllm" and args.native_sampler:
            environment["VLLM_USE_FLASHINFER_SAMPLER"] = "0"
        report["sampling_backend"] = "pytorch_native" if args.native_sampler else "engine_default"
        report["lmcache_cpu_offload"] = args.lmcache
        if args.lmcache:
            environment["LMCACHE_CONFIG_FILE"] = str(configuration)
            report["lmcache_max_cpu_gib"] = 0.25
        report["python_environment_cuda_toolkit"] = "CUDA_HOME" in environment
        log = dest.with_suffix(".backend.log")
        started = time.monotonic()
        with log.open("wb") as output:
            os.fchmod(output.fileno(), 0o600)
            server = subprocess.Popen(wrapper, cwd=root, env=environment, stdout=output, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + args.startup_seconds
            while True:
                try:
                    request = urllib.request.Request(endpoint + "/health", headers={"authorization": "Bearer " + key})
                    with opener.open(request, timeout=1): break
                except (OSError, urllib.error.URLError):
                    if server.poll() is not None: raise RuntimeError("backend exited before readiness; inspect private backend log")
                    if time.monotonic() > deadline: raise TimeoutError("backend readiness deadline exceeded")
                    time.sleep(.2)
            report["startup_seconds"] = time.monotonic() - started
            report["gpu_memory_ready_mib"] = gpu_memory()

            def infer(prompt, salt=None, origin=None, token=None):
                body = {"model": args.served_model, "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": 16, "temperature": 0, "stream": True,
                    "stream_options": {"include_usage": True}, "chat_template_kwargs": {"enable_thinking": False}}
                if salt is not None: body["cache_salt"] = salt
                request = urllib.request.Request((origin or endpoint) + "/v1/chat/completions", data=json.dumps(body).encode(),
                    headers={"content-type": "application/json", "authorization": "Bearer " + (token or key)})
                start = time.monotonic(); first = None; chunks = []; usage = None; count = 0
                with opener.open(request, timeout=45) as response:
                    for raw in response:
                        assert time.monotonic() - start < 90, "backend stream deadline exceeded"
                        count += len(raw)
                        assert count <= 1024**2, "backend stream exceeds limit"
                        if not raw.startswith(b"data:"): continue
                        data = raw[5:].strip()
                        if data == b"[DONE]": break
                        value = json.loads(data)
                        if value.get("usage"): usage = value["usage"]
                        for choice in value.get("choices", []):
                            content = choice.get("delta", {}).get("content")
                            if content:
                                if first is None: first = time.monotonic() - start
                                chunks.append(content)
                elapsed = time.monotonic() - start
                details = (usage or {}).get("prompt_tokens_details") or {}
                return {"request_ms": elapsed * 1000, "ttft_ms": None if first is None else first * 1000,
                    "usage": usage, "cached_tokens": details.get("cached_tokens", (usage or {}).get("cached_tokens")),
                    "nonempty": bool("".join(chunks).strip()), "answer_is_four": "".join(chunks).strip() == "4"}

            report["warmup"] = infer("Reply with only the number 4.")
            sizes = [64, 192, 320]; random.Random(0).shuffle(sizes)
            fixtures = random.Random(20261111)
            for size in sizes:
                for trial in range(args.trials):
                    prompt = f"fixture_{fixtures.getrandbits(128):032x}: " + "alpha " * size + "\nReply with only the number 4."
                    report["trials"].append({"prefix_words": size, "trial": trial,
                        "fresh": infer(prompt), "repeat": infer(prompt)})
            if args.engine == "vllm" and args.cache == "enabled":
                prompt = f"fixture_tenant_{fixtures.getrandbits(128):032x}: " + "alpha " * 320 + "\nReply with only the number 4."
                report["salt_trials"] = [infer(prompt, salt) for salt in ["public-tenant-a", "public-tenant-a", "public-tenant-b", "public-tenant-b"]]
            if args.coordinator_binary:
                from inference_runtime_smoke import rpc
                with socket.socket() as reserved:
                    reserved.bind(("127.0.0.1", 0)); coordinator_port = reserved.getsockname()[1]
                coordinator_state = root / "coordinator"
                coordinator_origin = f"http://127.0.0.1:{coordinator_port}"
                coordinator = subprocess.Popen([str(Path(args.coordinator_binary).resolve()), "--directory", str(coordinator_state),
                    "serve", "--listen", f"127.0.0.1:{coordinator_port}"], env={**environment, "CERYNTH_BENCH_BACKEND_KEY": key},
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                try:
                    deadline = time.monotonic() + 10
                    while not (coordinator_state / "control.sock").exists():
                        assert coordinator.poll() is None and time.monotonic() < deadline
                        time.sleep(.02)
                    rpc(coordinator_state, True, {"type":"configure", "endpoint":{"kind":"compatible", "id":"local", "config":{
                        "url":endpoint+"/v1", "cloud_consent":False,"credential_env":"CERYNTH_BENCH_BACKEND_KEY",
                        "cache_isolation":"vllm_per_grant_salt"}}})
                    grants = []
                    for _ in range(2):
                        session = str(uuid.uuid4())
                        grants.append(rpc(coordinator_state, True, {"type":"register", "grant":{"session_id":session,
                            "endpoint_id":"local","trust_domain":"public-fixture", "allowed_models":[args.served_model],
                            "concurrency_limits":[["session:"+session,1]],"max_priority":0}}))
                    prompt = f"fixture_coordinator_{fixtures.getrandbits(128):032x}: " + "alpha " * 320 + "\nReply with only the number 4."
                    observations = [infer(prompt, origin=coordinator_origin, token=grants[index]["token"]) for index in [0,0,1,1]]
                    assert [s["cached_tokens"] == 0 for s in observations] == [True,False,True,False], observations
                    try:
                        infer(prompt, salt="client-forged-namespace", origin=coordinator_origin, token=grants[0]["token"])
                        raise AssertionError("client-selected salt accepted")
                    except urllib.error.HTTPError as error: assert error.code == 400
                    rpc(coordinator_state, True, {"type":"revoke","session_id":grants[0]["session_id"]})
                    try:
                        infer(prompt, origin=coordinator_origin, token=grants[0]["token"])
                        raise AssertionError("revoked cache grant accepted")
                    except urllib.error.HTTPError as error: assert error.code == 401
                    report["coordinator_cache_isolation"] = {"samples":observations, "client_salt_rejected":True,
                        "revoked_grant_rejected":True,"cross_grant_first_request_cache_miss":True,
                        "same_grant_repeated_request_cache_hit":True, "cross_trust_domain_origin_binding_relaxed":False}
                finally:
                    coordinator.terminate()
                    try: coordinator.wait(timeout=10)
                    except subprocess.TimeoutExpired: coordinator.kill(); coordinator.wait()
            report["gpu_memory_after_mib"] = gpu_memory()
            checks = [sample for trial in report["trials"] for sample in [trial["fresh"], trial["repeat"]]]
            report["all_answers_four"] = all(sample["answer_is_four"] for sample in checks)
            report["success"] = all(sample["nonempty"] for sample in checks)
        except Exception as error:
            report["error_type"] = type(error).__name__
            report["error"] = str(error).replace(key, "<redacted>")
            raise
        finally:
            try:
                stopped = subprocess.run(["systemctl", "--user", "stop", unit], env=environment,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
                clean = stopped.returncode == 0
            except (OSError, subprocess.TimeoutExpired):
                clean = False
            if not clean:
                try:
                    loaded = subprocess.run(["systemctl", "--user", "show", unit, "--property=LoadState", "--value"],
                        env=environment, capture_output=True, text=True, timeout=5)
                    clean = loaded.returncode == 0 and loaded.stdout.strip() == "not-found"
                except (OSError, subprocess.TimeoutExpired):
                    clean = False
            try: server.wait(timeout=5)
            except subprocess.TimeoutExpired: server.kill(); server.wait()
            report["cleanup_scope_stopped"] = clean
            if log.stat().st_size <= 16 * 1024**2:
                log.write_text(log.read_text(errors="replace").replace(key, "<redacted>"))
            dest.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({"engine": args.engine, "cache": args.cache, "success": report["success"]}), flush=True)
            assert clean, "research scope cleanup could not be verified"


if __name__ == "__main__": main()
