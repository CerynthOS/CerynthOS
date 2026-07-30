# MIMIR — A Learned, Self-Improving CPU Scheduler for Linux

**Machine-learned Imitation for Managed, Introspective Runtime scheduling**

*Design & Research Proposal — v0.1*

> The name is a double pun: **Mímir** is the Norse well of wisdom whose water grants knowledge, and **"mimic"** points at the imitation-learning core of the system. MIMIR *drinks* from workload traces and *mimics* an offline oracle.

---

## 0. One-paragraph thesis

Operating-system CPU schedulers are still hand-tuned heuristics (Linux's default is EEVDF). They are general-purpose by necessity and therefore optimal for almost no specific workload. MIMIR replaces the *policy* — not the mechanism — with a learned model that is trained offline to imitate a hindsight oracle and refined with offline reinforcement learning, then deployed on a **real, production-grade Linux kernel** through `sched_ext`. It runs safely because `sched_ext` verifies the in-kernel code and automatically reverts to the default scheduler if the learned policy ever misbehaves. The result is a scheduler that *learns your machine's actual workloads*, keeps improving as they drift, and — critically — produces measurable, reproducible results against strong baselines instead of a toy kernel with nothing to schedule.

---

## 1. Why this idea, and why now

### 1.1 The problem with hand-tuned schedulers
A scheduler makes two decisions, constantly: **which** ready task to run next, and **for how long** (plus, on multicore, **on which CPU**). The default Linux scheduler encodes a fixed answer — a clever one, but one that cannot know that *this* machine spends its evenings compiling code while a game wants low frame-time latency, and its nights serving a latency-sensitive database. Every workload gets the same averaged-out heuristic.

Decades of systems research show that workload-specialized policies beat general ones. The catch has always been deployment: writing, testing, and safely shipping a custom in-kernel scheduler was so painful that almost nobody did it outside large companies.

### 1.2 What changed: `sched_ext`
`sched_ext` (SCX) is a mainline Linux scheduler class that lets you implement the scheduling policy as a BPF program and load/unload it **at runtime, without patching or rebooting the kernel**. As of 2026 it ships enabled by default on Fedora, Arch, CachyOS, NixOS unstable, and openSUSE Tumbleweed, and SteamOS uses an SCX scheduler during gameplay. Three properties make it the ideal substrate for learned scheduling:

1. **Real kernel, real workloads.** The policy schedules actual processes on real hardware, so the ML has a genuine optimization target — unlike a from-scratch kernel where the scheduler arbitrates between toy processes and the learning signal is meaningless.
2. **Fast iteration.** Swap the policy in seconds; no reboot. The ML dev-loop (collect → train → deploy → measure) becomes practical.
3. **Safety is built in.** The BPF verifier rejects memory-unsafe or non-terminating code, and if a loaded scheduler stalls a task for too long the kernel kills it and falls back to the default scheduler. This is *exactly* the guardrail an unproven learned policy needs — a bad model cannot brick the machine.

### 1.3 Why not build the whole OS from scratch
Building an OS from scratch to host "AI making OS decisions" fails for a subtle reason: **the ML starves.** A learned scheduler's value comes from optimizing real contention across dozens of real processes. A from-scratch kernel has no meaningful workload for months, so there is nothing for the model to learn from or beat. MIMIR inverts this: stand on a production kernel, get real workloads on day one, and spend the effort where the novelty is — the learning system.

---

## 2. Background & prior art (and where MIMIR sits)

MIMIR is not the first "learned systems" project; it is the first to combine a specific set of properties. Key precedents:

| Work | Subsystem | Approach | Runs on |
|---|---|---|---|
| **LinnOS** (OSDI '20) | SSD I/O | Tiny NN predicts slow I/Os; hedge | Real kernel |
| **Decima** (SIGCOMM '19) | Cluster (Spark) scheduling | Online RL | Simulator + Spark |
| **Parrot / "Imitation Learning for Cache Replacement"** (ISCA '20) | Cache eviction | Imitate Belady's *optimal* oracle | Simulator |
| **Learning Memory Access Patterns** (Hashemi, ICML '18) | Prefetching | LSTM sequence model | Simulator |
| **LLAMA / learned lifetime-aware allocation** (OSDI '20) | Memory allocation | Learned object lifetimes | Instrumented allocator |

Two takeaways:
- The **imitation-of-an-oracle** pattern (Parrot) is powerful and directly reusable — *but* caching has a clean provable optimum (Belady) and CPU scheduling does not, so MIMIR must construct a *best-effort* oracle rather than borrow a perfect one (see §4.3).
- Almost all of these run in **simulators or research prototypes**. MIMIR's distinguishing bet is **deployability on mainline Linux** via `sched_ext`, which turns "interesting in simulation" into "measurable on your laptop."

---

## 3. System architecture

MIMIR is a closed learning loop wrapped around the `sched_ext` mechanism. Five layers:

```
            ┌─────────────────────────────────────────────────────────────┐
            │                   USER SPACE (Rust + Python)                  │
            │                                                               │
            │   ┌───────────────┐   ┌──────────────┐   ┌───────────────┐    │
            │   │  Policy Model  │   │ Offline      │   │ Drift Monitor │    │
            │   │  (inference)   │◄──┤ Trainer      │◄──┤ + Model Zoo   │    │
            │   │  Rust: candle/ │   │ Python:      │   │ (MoE router)  │    │
            │   │  burn / tract  │   │ PyTorch, RL  │   └───────┬───────┘    │
            │   └──────┬─────────┘   └──────┬───────┘           │            │
            │          │ decisions          │ trains on         │ selects    │
            │          ▼                     ▼ traces           ▼            │
            │   ┌───────────────────────────────────────────────────────┐   │
            │   │        scx_rustland_core  (ring-buffer bridge)         │   │
            │   └───────────────┬───────────────────────▲───────────────┘   │
            └───────────────────┼───────────────────────┼───────────────────┘
                        dispatch │ order          metrics│ (features)
            ┌───────────────────▼───────────────────────┴───────────────────┐
            │                     KERNEL (BPF, C)                            │
            │   ┌──────────────┐   ┌───────────────┐   ┌─────────────────┐   │
            │   │ Telemetry /  │   │ Fast in-kernel│   │ Safety Guardrails│  │
            │   │ Feature      │──►│ policy tier   │   │ (starvation      │  │
            │   │ Collector    │   │ (int model /  │   │  watchdog, SCX   │  │
            │   │              │   │  decision tree)│   │  auto-fallback)  │  │
            │   └──────────────┘   └───────────────┘   └─────────────────┘   │
            └────────────────────────────────────────────────────────────────┘
```

### 3.1 Layer 1 — The mechanism (`sched_ext` + `scx_rustland_core`)
The unchanged kernel machinery: enqueue/dispatch hooks, dispatch queues, CPU selection primitives, and the automatic fallback. We do **not** reinvent this. `scx_rustland_core` gives us a producer–consumer bridge: the BPF side ships per-task metrics to user space over a ring buffer, and user space returns the dispatch order and per-task time slice. This bridge is MIMIR's integration seam.

### 3.2 Layer 2 — Telemetry / feature collector (BPF, C)
A BPF program attached to scheduler and tracepoint hooks that maintains per-task and per-CPU state and exports a compact **feature vector** per scheduling decision (see §4.2). It also logs full trajectories to user space for offline training. Must be cheap — feature extraction is on the hot path.

### 3.3 Layer 3 — The policy model (two-tier inference)
This is MIMIR's core design axis. In-kernel inference is fast but constrained (integer-only, verifier-bounded); user-space inference is rich but pays a kernel↔user round-trip. MIMIR supports **both tiers** and chooses per deployment:

- **Tier A — User-space model (default, easiest).** The policy lives in Rust user space (via `scx_rustland_core`) and can be a real neural net (`candle`/`burn`/`tract`) or gradient-boosted trees. Latency budget is looser (decisions are batched over the ready set), so model capacity can be high. `scx_rustland` already demonstrates that a user-space policy can *beat* EEVDF on interactive responsiveness despite the round-trip overhead — so this tier is viable, not just a toy.
- **Tier B — In-kernel model (advanced, lowest latency).** For per-task hot-path decisions at microsecond budgets, the trained model is compiled to a **branchless, integer-only** form that the verifier accepts: a quantized tiny MLP with fixed-point arithmetic, or (cleaner) a **decision tree / GBDT compiled to nested `if/else`**, or a **precomputed integer lookup table** keyed on discretized features. Trees are the sweet spot: they map naturally to BPF control flow and need no floating point.

The **model-to-BPF compilation path** for Tier B — turning a trained model into verifier-safe integer BPF — is one of MIMIR's genuine systems contributions (§6).

### 3.4 Layer 4 — Offline trainer (Python)
Runs entirely off the hot path. Consumes logged trajectories, computes oracle labels, trains via imitation + offline RL (§4), evaluates in **shadow mode** (§5.3), and — only after a candidate passes safety and performance gates — promotes it into the deployed policy. The deployed policy is always **frozen**; learning happens offline. This is a deliberate safety choice: no live gradient updates driving a running kernel.

### 3.5 Layer 5 — Safety guardrails
Three independent nets, in order of trust:
1. **`sched_ext` auto-fallback** — kernel reverts to EEVDF if any task stalls beyond the watchdog threshold. Free, and cannot be overridden by a bad model.
2. **Starvation watchdog + fairness floor (BPF)** — if any task's wait time crosses a bound, MIMIR overrides the model with a priority boost, guaranteeing progress regardless of what the model wants.
3. **Shadow mode + promotion gates (user space)** — new models run *without controlling anything*, logging their decisions and counterfactual outcomes, and are promoted only if they beat the incumbent on held-out workloads without violating fairness.

---

## 4. How the ML actually works (the heart of it)

### 4.1 What the model predicts (decision formulation)
For each ready task *t* given system state *s*, the model outputs:

- **A dispatch score** `π_score(t, s) → ℝ` — a ranking key that determines execution order (analogous to, and replacing, a hand-tuned virtual-runtime key).
- **A time slice** `π_slice(t, s) → {discrete quanta}` — how long *t* runs before re-evaluation.
- **(Multicore) a target CPU** `π_cpu(t, s) → cpu_id` — respecting cache locality, NUMA, and P-core/E-core asymmetry.

Formulating scheduling as **per-task scoring** (rather than "pick one action from a huge combinatorial space") keeps the model small and the output space clean, and matches exactly what `scx_rustland_core` expects back (an ordering + slice + CPU).

### 4.2 The state representation (features)
Compact, cheap to compute in BPF, and mostly counters/ratios:

**Per-task:** run/sleep ratio (interactivity signal), mean run-burst length, voluntary context-switch rate, nice/weight, time-since-last-run (starvation pressure), wakeup frequency, last CPU, syscall/I/O intensity proxy, resident-set/footprint proxy, cgroup id.

**System / global:** `nr_running`, `nr_queued`, per-CPU load, count of idle CPUs, NUMA node loads, P/E-core occupancy, and (for energy-aware variants) current DVFS/thermal state read from RAPL/`cpufreq`.

The feature set is deliberately overcomplete at first; feature-importance analysis (trivial for tree models) prunes it down to what fits the hot-path budget.

### 4.3 The oracle — the honest part
Parrot could imitate **Belady**, which is *provably optimal* for caching. **CPU scheduling has no such clean optimum** — minimizing general objectives is NP-hard, and objectives conflict (throughput vs. tail latency vs. fairness vs. energy). So MIMIR builds a **best-effort hindsight oracle**, not a perfect one, using information available offline but *not* online:

- **Per-objective hindsight solvers.** For tractable single objectives, use a policy that is optimal *for that objective with hindsight*. Example: **Shortest-Remaining-Processing-Time** is optimal for mean flow time on a single machine — using recorded true run lengths, SRPT-with-hindsight is a strong latency teacher. For makespan/throughput on short offline traces, use ILP or a strong greedy with full job knowledge.
- **Ensemble-best selection.** Run several strong existing schedulers (EEVDF, `scx_lavd`, `scx_bpfland`) over each recorded scenario and, per scenario, label the *best performer's* decisions as the target. The model learns to imitate "whichever expert would have won here."
- **Composite framing.** Because objectives conflict, the oracle is objective-weighted; the weights become an explicit knob (latency-biased vs throughput-biased profiles).

The teacher is therefore *strong but not provably optimal* — stated plainly so the research claims stay honest.

### 4.4 The learning algorithm (three stages)
Designed to match your background in RL, imitation, and agentic systems:

- **Stage A — Behavioral cloning (bootstrap).** Supervised imitation of the §4.3 oracle. Fast, stable, and yields a *working* policy quickly. This is the MVP model and the safety baseline.
- **Stage B — Offline reinforcement learning.** Refine beyond the oracle using logged trajectories and a **composite reward** — a weighted mix of throughput, p99/p999 latency, fairness (e.g., Jain's index), and energy (perf-per-watt). Use *offline* RL algorithms (e.g., **CQL / IQL**, or a **Decision Transformer**) precisely because **online exploration in a live scheduler is dangerous** — a bad exploratory action tanks the whole machine. Offline RL learns from logged data without risky live trial-and-error.
- **Stage C — DAgger-style distribution-shift correction.** A cloned policy drifts into states the oracle never demonstrated. Deploy the current policy **safely (with fallback + shadow mode)**, log the states it actually visits and where its outcomes are poor, query the oracle on those states, aggregate, and retrain. This closes the classic imitation-learning gap.

### 4.5 How it keeps learning (online adaptation without online risk)
The deployed model is frozen, but the *system* adapts:

- **Concept-drift detection.** The drift monitor watches feature/outcome distributions; when the live workload diverges from training distribution, it triggers a retrain on fresh traces.
- **Mixture-of-Experts + safe policy selection.** Instead of one model, maintain a **zoo of vetted specialist policies** (e.g., "compile-heavy", "latency-serving", "interactive/gaming"). A lightweight **workload classifier** routes to the right expert, and a **contextual bandit** performs *safe* online selection *among already-vetted policies only* — adaptation without ever running an unvetted policy. This gives online responsiveness while keeping the "never deploy something unproven" guarantee.

### 4.6 Why the model can be tiny (and must be)
Scheduler decisions happen on microsecond budgets. LinnOS made the same call: use a deliberately tiny network. MIMIR's Tier-B in-kernel models are small MLPs / shallow trees / LUTs; capacity lives in Tier A (user space) where the budget allows it. A learned scheduler that is *accurate but slow* is a net loss — inference latency is part of the objective, and MIMIR measures it (§5).

---

## 5. How we prove it works (evaluation)

Research credibility lives or dies here.

### 5.1 Baselines
EEVDF (kernel default), `scx_rustland` (same substrate, hand-tuned), `scx_lavd`, `scx_bpfland`.

### 5.2 Workloads (diverse on purpose, to avoid benchmark overfitting)
- **schbench** — scheduler wake-up latency.
- **Parallel kernel compile** — throughput under load.
- **Redis / YCSB** — database tail latency.
- **nginx + wrk** — web-serving p99.
- **Mixed interactive + batch** — a latency-sensitive app alongside a CPU hog (the classic "game + compile" scenario `scx_rustland` targets).
- **Workload-shift scenario** — abruptly change the mix to test drift adaptation.
- **Held-out workloads** — trained-on vs tested-on split to measure generalization.

### 5.3 Metrics
Throughput; p50/p99/p999 latency; fairness (Jain's index / max-min); **scheduler CPU overhead**; **model inference latency**; energy (RAPL) and perf-per-watt. The two bolded ones are non-negotiable — a scheduler that wins on latency but burns cores or adds inference lag is not a real win, and honest overhead accounting is what separates a credible result from a demo.

### 5.4 Ablations
BC-only vs +offline-RL vs +DAgger; Tier-A vs Tier-B inference; single model vs MoE; with/without drift adaptation. Plus **shadow-mode counterfactual evaluation** to compare a new model's would-be decisions against the incumbent's *actual* outcomes on identical workloads.

---

## 6. Key features — what MIMIR actually introduces

Twelve concrete capabilities. The **[N]** tag marks the ones that are genuinely novel research contributions (vs. solid engineering).

1. **Learned CPU scheduling policy on mainline Linux** — deployable on a real kernel via `sched_ext`, not a simulator or research OS. **[N — deployability]**
2. **Two-tier inference (user-space rich model + in-kernel integer model)** with a principled latency/accuracy switch. **[N]**
3. **Model-to-BPF compiler** — turns a trained tree/quantized-MLP into verifier-safe, floating-point-free BPF for hot-path inference. **[N — systems contribution]**
4. **Best-effort hindsight oracle for scheduling** (per-objective solvers + ensemble-best labeling) to enable imitation where no Belady-style optimum exists. **[N]**
5. **Offline-RL refinement with a composite reward** (throughput + tail latency + fairness + energy) — avoiding dangerous online exploration. **[N]**
6. **DAgger loop for distribution-shift correction**, run safely under fallback + shadow mode.
7. **Concept-drift detection with automatic retraining** — the scheduler tracks workload change and updates itself. **[N — self-improving on real hardware]**
8. **Mixture-of-Experts policy zoo + workload router**, with a **contextual bandit for safe online selection** among vetted policies only. **[N]**
9. **Layered safety** — `sched_ext` auto-fallback + BPF starvation watchdog + fairness floor + shadow-mode promotion gates.
10. **Full telemetry / feature-collection framework** in BPF, reusable for other learned-subsystem research.
11. **Overhead-honest evaluation harness** — measures scheduler CPU cost and inference latency as first-class metrics; reproducible across workloads. **[N — reproducible benchmark for learned schedulers]**
12. **Objective-profile knob** — latency-biased vs throughput-biased vs energy-biased policies from the same framework, selectable per deployment.

**Count:** 12 key features, of which **8 carry a genuine research-novelty claim.**

---

## 7. Novel research contributions (the publishable core)

Stated conservatively and positioned against prior art:

1. **The first safe, self-retraining learned CPU scheduler deployable on mainline Linux.** Prior learned schedulers (Decima) are cluster-level and simulator/RL-online; MIMIR is OS-level, on real hardware, with hard safety guarantees.
2. **A model-to-BPF hot-path inference path** under verifier + integer-only constraints — a reusable systems technique for *any* in-kernel learned control, not just scheduling.
3. **An oracle-construction method for a domain without a provable optimum** (unlike Parrot/Belady), combining per-objective hindsight solvers with ensemble-best imitation.
4. **A safety architecture for learned kernel policies** — the combination of verifier guarantees, auto-fallback, watchdog/fairness floors, and shadow-mode promotion, which lets ML drive the kernel without risk of bricking it.
5. **Workload-adaptive scheduling via a safely-selected expert mixture** with drift-triggered retraining — online adaptivity without online-learning danger.
6. **An open, overhead-honest benchmark harness** for learned schedulers on `sched_ext`, addressing the reproducibility gap in learned-systems research.

**Honest caveat:** several contributions are *novel combinations / first-deployable-on-mainline* rather than fundamentally new ML algorithms. That is exactly the kind of contribution systems venues (OSDI, SOSP, ATC, EuroSys) value — deployability + real evaluation + safety — so it is a strength, not a weakness, for this project's target audience.

---

## 8. Implementation roadmap (phased, realistic for a student part-time)

| Phase | Goal | Deliverable | Rough effort |
|---|---|---|---|
| **P0** | Environment + baseline | SCX-enabled kernel running; `scx_rustland` loads; EEVDF benchmarked on the workload suite | 1–2 wks |
| **P1** | Telemetry & data pipeline | BPF feature collector; full trajectory logging; trace dataset across all workloads | 2–3 wks |
| **P2** | Oracle + behavioral cloning | Hindsight oracle; first learned **user-space** policy; ties/beats EEVDF on ≥1 workload | 3–4 wks |
| **P3** | Offline RL + DAgger | Composite-reward offline-RL policy; DAgger loop closing the shift gap | 4–6 wks |
| **P4** | In-kernel inference (Tier B) | Model-to-BPF compiler; integer/quantized hot-path model; latency vs Tier A measured | 3–4 wks |
| **P5** | Drift + MoE | Drift detector + retrain loop; policy zoo + router + safe bandit selection | 4–6 wks |
| **P6** | Full evaluation + write-up | All ablations, overhead accounting, held-out generalization, paper/report | 3–4 wks |

**Total: ~6–8 months part-time.** P0–P2 alone already produce a demoable, portfolio-worthy result (a learned scheduler that beats the default on a real workload).

---

## 9. Technical challenges & mitigations

| Challenge | Why it's hard | Mitigation |
|---|---|---|
| Hot-path latency budget | µs-scale decisions; a big model is too slow inline | Tier-B tiny quantized model / tree / LUT; push capacity to Tier-A user space for coarser decisions |
| **BPF has no native floating point** | Kernel BPF can't freely use floats | Fixed-point integer arithmetic; integer-only decision trees; precomputed integer LUTs |
| BPF verifier complexity limits | Bounded instructions/loops | Small, bounded, unrolled inference; CO-RE; trees compile to shallow branch chains |
| Distribution shift | Cloned policy visits unseen states | DAgger loop + drift detection + retrain + shadow mode |
| Reward specification / reward hacking | Composite objectives conflict; RL exploits loopholes | Hard guardrails + fairness floor + objective-weighted reward + shadow-mode vetting |
| A bad policy tanks the machine | ML driving the kernel is scary | `sched_ext` auto-fallback + starvation watchdog + promote only after gates |
| Benchmark overfitting | Winning on schbench ≠ general win | Diverse suite + train/test workload split + held-out generalization test |
| Hardware variance / reproducibility | Results depend on the box | Pinned kernel/config, documented hardware, multiple runs + variance reporting |

---

## 10. Tech stack

- **Kernel:** mainline SCX-enabled distro (Fedora / Arch / CachyOS). Configs: `CONFIG_SCHED_CLASS_EXT`, `CONFIG_BPF`, `CONFIG_BPF_JIT`, `CONFIG_DEBUG_INFO_BTF`.
- **Scheduler framework:** `scx_rustland_core` (user-space Rust policy — the starting template); raw `sched_ext` BPF (C) for the Tier-B in-kernel model.
- **Training:** Python + PyTorch; offline-RL via `d3rlpy` / CORL-style implementations.
- **Inference:** Rust — `candle` / `burn` / `tract` for Tier A; custom integer inference + model-to-BPF codegen for Tier B.
- **Tooling:** `libbpf` + BPF CO-RE, `clang 18+`, `bpftool`, `drgn` (`scx_show_state`), `perf`, `ftrace`, RAPL (energy), `schbench` / `wrk` / YCSB (workloads).
- **Experiment tracking:** Parquet traces + MLflow or Weights & Biases; pinned configs for reproducibility.

---

## 11. Stretch goals & future features

MIMIR is a scheduler first, but the framework generalizes to a broader **learned OS**:

- **Learned I/O latency prediction** (LinnOS-style) integrated into the same telemetry/safety fabric.
- **Learned prefetch / readahead** (sequence models over access patterns).
- **Learned page-cache eviction** — and here you *can* borrow Belady as a true oracle (Parrot-style), unlike scheduling.
- **Learned DVFS / CPU-frequency control** for energy — and then **cross-subsystem co-design**: a shared representation coordinating scheduler + DVFS + I/O, which is genuinely unexplored territory.
- **Per-application specialization** — auto-detect the running app and load its specialist policy from the zoo.
- **GPU-aware scheduling** — `sched_ext` already has GPU-awareness on its roadmap; a MIMIR variant tuned for ML-serving hosts brings this full circle to AI-workload optimization.
- **Fleet learning** — federated policy learning across many machines, so each box benefits from the fleet's experience without sharing raw traces.

---

## 12. Glossary

- **`sched_ext` / SCX** — Linux scheduler class allowing BPF-defined, runtime-loadable scheduling policies.
- **BPF / eBPF** — in-kernel virtual machine for safe, verified programs; the mechanism SCX uses.
- **CO-RE** — "Compile Once, Run Everywhere"; BPF portability across kernels.
- **EEVDF** — the current default Linux scheduler.
- **Behavioral cloning** — supervised imitation of expert decisions.
- **Offline RL** — reinforcement learning from a fixed logged dataset, no live exploration.
- **DAgger** — Dataset Aggregation; iteratively fixes imitation distribution shift by querying the oracle on the learner's own visited states.
- **MoE** — Mixture of Experts; multiple specialist models with a router.
- **Concept drift** — change in the data distribution over time (here, changing workloads).

---

## 13. Selected references (by name)

- Heo et al., *sched_ext* — Linux kernel documentation & LWN coverage; `sched-ext/scx` repository (`scx_rustland`, `scx_rustland_core`).
- Hao et al., **LinnOS: Predictability on Unpredictable Flash Storage** (OSDI 2020).
- Mao et al., **Learning Scheduling Algorithms for Data Processing Clusters** (Decima, SIGCOMM 2019).
- Liu et al., **An Imitation Learning Approach for Cache Replacement** (Parrot, ISCA 2020).
- Hashemi et al., **Learning Memory Access Patterns** (ICML 2018).
- Maas et al., **Learning-based Memory Allocation** (LLAMA, OSDI 2020).
- Ross et al., **A Reduction of Imitation Learning and Structured Prediction to No-Regret Online Learning** (DAgger, AISTATS 2011).
- Kumar et al., **Conservative Q-Learning for Offline RL** (CQL, NeurIPS 2020).

---

*End of v0.1. This is a living document — the roadmap phases are the natural places to revise as real numbers come in.*
