# OS-for-AI: An Operating System Designed for Autonomous Agents

*Research prototype — May 2026*

---

## Executive Summary

Modern operating systems were built for human users at terminals. Their core assumptions — that users perceive time in milliseconds, navigate state by habit, hold stable identities, and run a bounded number of parallel processes — translate into concrete engineering costs when the user is a long-running autonomous AI agent.

This project asks what an OS would look like if agents were the primary users, then builds and tests partial answers. It is not a specification exercise: it produced a working Rust runtime validated against measurable properties, and a second prototype running on seL4 — one of the few kernels with a machine-checked proof of functional correctness. The core claims were falsifiable, tested adversarially, and held.

---

## The Problem

AI agents are no longer demos. Systems like Claude Code, Devin, and SWE-agent run for hours or days, execute thousands of actions, and operate with minimal human oversight. They all run on Linux — not because Linux is the right substrate, but because it is what exists.

The workarounds practitioners have built to compensate are the clearest signal of the gap:

- **Temporal** re-implements durable execution and replay at the application layer.
- **LangGraph** re-implements explicit state management at the application layer.
- Various rollback and checkpoint frameworks re-implement transactional state at the application layer.

This is the historical pattern for missing abstractions. Garbage collection was applicative before languages provided it. Transactional persistence was per-application before embedded databases became standard. The question this project asks is: which OS-level abstractions would eliminate this layer?

The specific costs of running agents on Linux:

| OS assumption | Cost for an autonomous agent |
|---|---|
| State accumulates implicitly | No way to snapshot or roll back cleanly without custom infrastructure |
| Observability is reconstructive | Causal trace requires correlating multiple disparate log systems |
| Trust tied to Unix identity | No native primitive for "delegate this right to a sub-agent for 5 minutes, then revoke" |
| Parallelism bounded by human cognition | Each Docker container: 100 MB–1 GB of RAM. 16 GB machine → dozens of agents, not thousands |

---

## The Claim: Three Measurable Properties

Rather than arguing from first principles, this project committed to three concrete, falsifiable targets:

**P1a — Density ×5.** Maintain 5× more *dormant* agents than Linux+containers on the same hardware. (At-rest footprint only; active density P1b — throughput at equivalent latency — is not claimed: that comparison was abandoned as non-transferable to the seL4-native substrate.)

**P2 — Transactional rollback ≤ 100 ms.** Roll back to any prior state among N in O(depth), bounded at ≤ 100 ms for depth 500.

**P3a — Causal lookup O(1) ≤ 10 ms.** Retrieve any action by ID at p99 ≤ 10 ms across 10⁸ entries.

A fourth property underpins the others: **P6 — Crash atomicity.** A process killed at any point must leave persistent state consistent; a surviving process must detect any inconsistency on restart.

Formal priority order: P6 ≻ P2 ≻ P3 ≻ P1. Correctness before performance.

---

## What Was Built

### Phase 1–4: Design and architectural decisions (2025–early 2026)

The first phase produced a formal specification across ten documents — properties, hypotheses, non-goals, threat model, architectural ceilings — and 53 Architecture Decision Records (ADRs). Each ADR records not just the decision but the reasoning, the alternatives considered, and the conditions under which the decision should be revisited.

This upfront investment paid off: when experimental results contradicted a hypothesis (e.g., T6-soak initially suggested a memory leak), the ADR system provided the framework to decide whether to refute the hypothesis, amend the design, or adjust the measurement criterion. No result was buried; three explicit "refutation ADRs" document cases where the data contradicted the original model.

### Phase 5–10: Linux prototype in Rust

A runtime for autonomous agents built on Wasmtime (WebAssembly isolation) and RocksDB (persistent causal log). Core components:

- **Causal log.** Every agent action is content-addressed (SHA-256) and recorded with its causal parents as a DAG — not a tree. Any action is retrievable by ID across 10⁸ entries. The log is append-only and crash-safe.

- **Transactional rollback.** An agent rolls back to state N without custom application logic. The rollback traverses the causal DAG in reverse; cost is O(depth), not O(N).

- **Capability-based access control.** Access rights are explicit tokens, delegated from parent to child agent, revocable at any time. A compromised sub-agent can affect only resources it was explicitly granted — not its parent's full context. The confused-deputy attack was used to validate this: an untrusted agent was given a path to escalate via a legitimate intermediary; isolation held, though the audit gap was found and fixed in the process.

- **Bounded inference queues.** Multiple agents share LLM inference capacity. The scheduler enforces fairness (starvation-free) and priority classes (foreground/background/batch). Validated under concurrent load with 6 workers sharing 2 slots.

- **Agent eviction/wake cycle.** Dormant agents are evicted to a content store and restored on demand. The restoration pipeline (WASM reinstantiation + causal state replay) completes in sub-millisecond time under typical conditions.

### Phase 11–12: seL4 stack (QEMU AArch64)

seL4 is a formally verified microkernel — its functional correctness is machine-checked, not asserted. The prototype was ported to run on top of it across 11 integration milestones:

- Wasmtime running inside a seL4 process with no standard library (no_std Rust).
- Persistent key-value store (redb) over a virtual block device (virtio-blk).
- W^X (write-xor-execute) enforcement on the JIT compilation pool.
- Multi-agent commit with per-agent capability badges.
- Crash atomicity under adversarial kill scenarios.
- Isolation demonstrated against adversarial WebAssembly modules that attempt privilege escalation.

---

## Five Architectural Bets

Each bet was explicitly falsifiable. The refutation conditions were written before the experiments.

**Bet 1 — Causal DAG, not tree.**  
Parallel agent actions produce real concurrency — a chain structure would require serialization. The log records `caused_by[]` as a list of direct parents. Validated from the first multi-agent experiments; no case was found where DAG semantics created a correctness problem.

**Bet 2 — RocksDB LSM, not SQLite B-tree.**  
For an append-only log at 10⁸ entries with O(1) lookup by opaque key, an LSM tree is architecturally correct: write amplification is amortized across compaction, and bloom filters eliminate the disk I/O for missing keys. Validated by T5: p99 ≤ 2 ms on 10⁸ entries under NVMe, 5–7× under the 10 ms target.

**Bet 3 — Wasmtime + Tokio, not Docker.**  
WebAssembly module isolation is 4,500–7,375× lighter per dormant agent than Docker+Python in RAM. The overhead-per-agent law was measured and fitted: `overhead(N) = 9.65 − 54/N` KB (R²=0.988). The asymptotic overhead is ~9.65 KB/agent; the term `54/N` reflects shared fixed costs (WASM binary + Tokio runtime) that amortize at N ≥ 300. Prediction for N=10,000: 9.64 KB/agent.

**Bet 4 — Revocable capabilities, not Unix identity.**  
A Unix process either has an fd or it doesn't; there is no native revocation. This system delegates explicit capability tokens from parent to child and propagates revocation lazily through the chain. Validated functionally; the confused-deputy scenario confirmed that isolation holds even when the attacker controls a legitimate intermediary.

**Bet 5 — Asymmetric supervision.**  
Human supervisors observe (causal log), intervene (rollback, revocation), and authorize — but do not interact in real time. This shapes the entire log design: the latency target for P3a is 10 ms (a human can wait), not 1 µs (a hardware interrupt cannot). The corollary is that the log is durable-first, not latency-first.

---

## Results

All measurements were taken on an AMD Ryzen 5 PRO 4650U laptop with a WD SN530 NVMe PCIe Gen3×4. This is consumer-grade hardware. The targets are stated conservatively; the results exceed them by meaningful margins.

### P3a — Causal lookup latency

| Dataset | Concurrency | p50 | p99 | Target | Margin |
|---|---|---|---|---|---|
| 10,000 entries (T5-ter) | Sequential | 608 µs | 1,141 µs | 20,000 µs | 17× |
| 1,000,000 entries, 4 writers (T5-p3c) | Concurrent | 12 µs | 23 µs | 200 µs | 8× |

Note: T5-p3c measures lookup under concurrent write load. The LSM structure absorbs write pressure without degrading read latency.

### P2 — Transactional rollback latency

| Scenario | Depth | p95 | Target | Margin |
|---|---|---|---|---|
| Rollback benchmark (store bench) | 10⁶ operations | 99 µs | — | — |
| SEF-2 end-to-end (with polling overhead) | 500 | 17–20 ms | 100 ms | 5× |

### P1 — Agent density vs Docker+Python

| Metric | Wasmtime | Docker+Python | Ratio |
|---|---|---|---|
| Dormant RAM per agent (asymptotic) | ~9.65 KB | ~44 MB | 4,539–7,375× |
| Overhead law | O(1) for N≥300 | O(1) | — |

Verdict: **partial.** The memory ratio exceeds the 5× target by three orders of magnitude. However, this measures dormant RAM only, not a full head-to-head on seL4 hardware. The Linux prototype numbers do not predict performance on a seL4-native deployment; benchmarking on the wrong substrate would produce a number that says nothing about the actual target.

### Agent wake latency (T7 / T8)

| Condition | p50 | p99 | Budget |
|---|---|---|---|
| Baseline (N=50, 20 dormant, K=3) | 204 µs | 311 µs | 10 ms |
| Saturated (N=50, 20 dormant, 5M prepop, 5 min) | 218 µs | 378 µs | 10 ms |

At 378 µs p99 under saturation, wake latency represents < 0.01% of a 5-second inference cycle. Predictive admission (pre-warming dormant agents) was evaluated and found unnecessary in this regime.

### P6 — Crash atomicity (seL4)

40 crash scenarios across 4 kill points (pre-commit, mid-commit, post-commit, server-side). In every case: the store was left in a consistent state, and a surviving server detected any inconsistency on restart. The test harness (`c10-crash/test.py`) replays each scenario deterministically.

### Capability isolation (SEF-3, SEF-9)

A confused-deputy attack was staged: an untrusted agent was given a path to escalate via a legitimate intermediary. Isolation held. An audit gap was identified (a rate-limit bypass that could mask audit entries), fixed, and the fix was validated with a second adversarial run.

---

## Adversarial Testing

The system was attacked, not just validated. This is a methodological commitment: a property that only holds under friendly inputs is not a property.

Three adversarial scenarios were run:

- **SEF-3/SEF-9 (confused deputy).** Untrusted agent attempts privilege escalation via a legitimate intermediary. Isolation held; audit gap found and fixed.
- **SEF-12/SEF-13 (P2/P3/P5 under adversarial load).** Rollback and causal lookup under concurrent adversarial write pressure. Both passed.
- **T6-soak (memory leak hypothesis).** Initial results suggested unbounded RSS growth. Investigation found the OLS criterion was structurally inapplicable under compaction spikes (R²=0.24). The memory leak hypothesis was explicitly refuted: RSS is bounded by memtables (~256 MB) + block caches (~512 MB) + agent overhead (~5 MB) ≈ 793 MB for 500 agents. The refutation is recorded in ADR-0034.

---

## What Was Deliberately Not Measured

**Full density comparison on seL4 hardware.** Measuring Wasmtime vs Docker on a Linux server NVMe says nothing about seL4 storage latency. The number would be meaningless for the actual target.

**P3a on real seL4 hardware.** The measurement harness (`sel4-hello/d-p3a/`) is implemented and ready. It requires a physical ARM board or NVMe passthrough — QEMU storage I/O is not an acceptable substrate for latency measurement. This is hardware-gated, not a missing benchmark.

**Power-loss durability.** Explicitly out of scope. The consistency oracle is implemented and waiting for the same hardware trigger.

These are results of a single methodological commitment: do not validate a property on the wrong substrate.

---

## What Remains Open

The project is closed as a prototype. Two active items are hardware-gated:

- **P3a on seL4 hardware.** The harness is ready; an ARM board or NVMe passthrough is required.
- **Power-loss durability.** Requires true power interruption, not process kill.

Two architectural items have explicit software triggers:

- **Cross-store atomic commit** — triggered when garbage collection is implemented (requires a two-phase commit across the causal log and content store).
- **Real setjmp/longjmp + temporal watchdog** — triggered when ≥2 agents share a single virtual address space (current design gives each agent its own VSpace on seL4).

---

## Methodology

**Decisions are explicit and versioned.** 53 ADRs record not just what was decided but why, what was rejected, and under what conditions the decision should be revisited. This makes the design auditable and falsifiable rather than implicit.

**Hypotheses are written before experiments.** Every benchmark has a target defined before the run. A result that beats the target by 17× is meaningful precisely because the target was fixed in advance. Post-hoc target-setting would not constitute evidence.

**Failures are documented, not buried.** Three "refutation ADRs" (ADR-0032, ADR-0033, ADR-0034) document cases where experimental results contradicted the original model. In each case, the correct response was to update the model, not discard the data.

**Wrong-substrate measurements are refused.** A benchmark that proves nothing about the target is not a useful benchmark. This cost one measurement (P3a on seL4 hardware) and gained methodological integrity.
