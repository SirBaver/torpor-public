# RUNBOOK — Tests unitaires et scénarios de qualification

Ce document liste tous les runners disponibles, leurs dépendances WASM, les commandes
exactes pour les reproduire, et les critères d'acceptation.

**Principe :** les runners SEF (qualification formelle) ont un exit code machine-vérifiable.
Les runners de cas d'usage (démo) s'évaluent sur leur sortie textuelle.

> **Créer un nouvel agent ou un nouveau runner ?** Ce RUNBOOK reproduit les runners
> *existants*. Pour le geste de création (où écrire, quel squelette, comment vérifier),
> voir le how-to [`docs/guides/howto-agent-flotte.md`](../docs/guides/howto-agent-flotte.md).

---

## Prérequis

```bash
# Variable d'environnement requise pour RocksDB sous GCC récent
export CXXFLAGS="-include cstdint"

# Cible WASM
rustup target add wasm32-unknown-unknown

# Modèle Ollama (tous les runners LLM)
ollama pull llama3.2:3b       # recommandé (rapide, ~2 GB)
ollama pull mistral:7b         # alternatif (meilleur, ~4 GB)
```

Runners **sans Ollama** (déterministes) : `sef1`, `sef2`, `sef4`, `sef5`, `sef6`,
`sef12`, `sef13`, `s11`, `icsr-writer`, `icsr-verifier`.

---

## Build WASM

Tous les runners chargent des `.wasm` depuis `target/wasm32-unknown-unknown/release/examples/`.
Le répertoire de travail doit être `poc/`.

```bash
cd poc/

# Modules utilisés par la majorité des runners
cargo build --target wasm32-unknown-unknown -p agent-sdk \
  --example task_step --example multi_turn --release

# Modules spécialisés (use cases concrets)
cargo build --target wasm32-unknown-unknown -p agent-sdk --release \
  --example quality_writer    \   # self-correct-runner
  --example approval_agent    \   # approval-runner
  --example data_accessor     \   # capability-runner
  --example brainstorm_synth  \   # brainstorm-runner
  --example llm_worker        \   # supervisor-runner
  --example llm_supervisor    \   # supervisor-runner
  --example code_reviewer     \   # code-review-runner
  --example severity_judge    \   # code-review-runner
  --example voter_agent       \   # consensus-runner
  --example tally_secretary   \   # consensus-runner
  --example critic_agent      \   # iterative-runner
  --example incident_aggregator \ # incident-runner
  --example support_triage    \   # support-runner
  --example orchestrator          # orchestrate-runner

# Build one-liner pour tout
cargo build --target wasm32-unknown-unknown -p agent-sdk --release
```

---

## 1. Tests de qualification SEF (déterministes, exit code)

Ces runners n'ont pas besoin d'Ollama. **Exit 0 = PASS, exit 1 = FAIL.**

### SEF-1 — Persistance d'état après redémarrage (P1a)

**Propriété :** après N actions + arrêt propre, réouverture → état identique.
**Contrat :** P-α (count identique), P-β (hash final identique), P-γ (snapshot récupérable), P-δ (last_action_id identique).

```bash
cargo run --bin sef1-runner
# Expected: exit 0 — "0 = pass (P-α, P-β, P-γ, P-δ tiennent)"
```

---

### SEF-2 — Rollback transactionnel (P2)

**Propriété :** rollback à l'action n°500 → état hash identique à celui d'après l'action 500 originale.
**Contrat :** P-α (hash_after identique), P-β (seq restored), P-γ (log append-only après rollback), P-δ (durée rollback ≤ borne).

```bash
cargo run --bin sef2-runner
# Expected: exit 0 — "0 = pass (4 propriétés + budget rollback OK)"
```

---

### SEF-4 — Atomicité crash SIGKILL (P6)

**Propriété :** un crash brutal (SIGKILL simulé) pendant une écriture → état post-recovery ∈ {pré-action, action-committée}.
**Contrat :** aucun état "à moitié écrit" observable.

```bash
# Nécessite la feature crash-injection
CXXFLAGS="-include cstdint" cargo build --features crash-injection \
  --bin sef4-victim --bin sef4-verify

TMPDIR=$(mktemp -d)
./target/debug/sef4-victim "$TMPDIR" 100 50  # 100 actions, crash à l'action 50
./target/debug/sef4-verify "$TMPDIR"
# Expected: sef4-verify exit 0 — "exit 0 = pass"
```

---

### SEF-5 — Traçabilité causale (P3a)

**Propriété :** pour tout `action_id` retourné par `append`, `get(action_id)` produit le bon contenu en ≤ 10 ms (p99), 100 % de complétude.
**Contrat :** P-α (p99 latence ≤ 10 ms), P-β (complétude 100 %).

```bash
cargo run --bin sef5-runner
# Expected: exit 0 — "0 = pass (P-α p99 ≤ 10 ms  ET  P-β complétude 100%)"
# Note: génère ~15 GB dans /tmp — supprimer après: rm -rf /tmp/sef5-*
```

---

### SEF-6 — Déterminisme de transition d'état (P5)

**Propriété :** deux instances avec le même `LogicalClock` et les mêmes messages produisent le même hash final et la même séquence d'`action_id`.
**Contrat :** P-α (hash final identique), P-β (séquence action_ids identique), P-γ (snapshots identiques).

```bash
cargo run --bin sef6-runner
# Expected: exit 0 — "0 = pass (P-α, P-β, P-γ tiennent)"
```

---

### SEF-12 — Rollback adversarial (P2 campagne)

**Propriété :** variantes adversariales de P2 : rollback², rollback + flood immédiat, liveness sous charge.
**Variantes :** V2.2 (rollback²), V2.3 (rollback + flood), V2.4 (liveness).

```bash
cargo run --bin sef12-runner
# Expected: toutes les assertions P-α/P-β/P-γ/P-δ affichent "pass" ou "PASS"
# Sortie: "[sef12/V2.2] SEF-12 V2.2 PASS" etc.
```

---

### SEF-13 — Traçabilité causale adversariale (P3 campagne)

**Propriété :** résistance aux forgeries d'`action_id` ; intégrité content-addressed.
**Variantes :** V3.3a (faux positifs = 0), V3.3b (pas de violations d'intégrité), V3.4 (non-constructible).

```bash
cargo run --bin sef13-runner
# Expected: "verdict V3.3a : PASS", "verdict V3.3b : PASS", "verdict V3.4 : PASS"
```

---

## 2. Scénarios Scheduler

### S11 — Éviction / réveil (déterministe, sans Ollama)

**Propriété :** éviction → dormant → wake → continuité causale.
**Contrat :** P-α (dormant_count correct), P-β (aucune action perdue), P-γ (causalité préservée).

```bash
cargo run --bin s11-runner
# Expected: sortie "P-α PASS", "P-β PASS", "P-γ PASS"
```

---

### S10 — Scheduler unifié C1+C2 (nécessite Ollama)

**Propriété :** borne I/O + borne inférence + priorité simultanément.
**Contrat :** P-α (max I/O ≤ cap_io), P-β (max inférence ≤ k_infer), P-γ (Supervisor > Foreground).

```bash
CXXFLAGS="-include cstdint" cargo run --bin s10-runner -- llama3.2:3b
# Expected: "P-α PASS", "P-β PASS", "P-γ PASS"
```

---

### S12 — SchedulerCoordinator réveil à la demande (nécessite Ollama)

**Propriété :** réveil ciblé d'agents dormants sans passer par la file I/O normale.
**Contrat :** P-α (n_woken == n_dormant), P-β (borne C2 respectée), P-γ (bypass C2 actifs).

```bash
CXXFLAGS="-include cstdint" cargo run --bin s12-runner -- llama3.2:3b
# Expected: "P-α PASS", "P-β PASS", "P-γ PASS"
```

---

## 3. Durabilité crash I-CSR (sans Ollama)

**Propriété :** Invariant de Cohérence Store–Réveil : après coupure (drop propre ou exit brutal), les entrées du log correspondent aux commits du store.

```bash
TMPDIR=$(mktemp -d)
WITNESS="$TMPDIR/witness.json"

# Phase écriture + coupure drop
CXXFLAGS="-include cstdint" cargo run --bin icsr-writer -- \
  "$TMPDIR/store" "$TMPDIR/log" 500 "$WITNESS" drop

# Vérification
cargo run --bin icsr-verifier -- "$TMPDIR/store" "$TMPDIR/log" "$WITNESS"
# Expected: exit 0 — "I-CSR PASS"

# Variante SIGKILL simulé
TMPDIR2=$(mktemp -d)
WITNESS2="$TMPDIR2/witness.json"
CXXFLAGS="-include cstdint" cargo run --bin icsr-writer -- \
  "$TMPDIR2/store" "$TMPDIR2/log" 500 "$WITNESS2" exit
cargo run --bin icsr-verifier -- "$TMPDIR2/store" "$TMPDIR2/log" "$WITNESS2"
# Expected: exit 0 — "I-CSR PASS"
```

---

## 4. Qualification inférence P10 (nécessite Ollama)

### P10-S3 — Borne dure d'inférence

**Propriété :** pool_cap=2, N workers → jamais plus de 2 inférences simultanées.
**Contrat :** P-α (max_concurrent ≤ pool_cap), P-β (aucune perte de requête), P-γ (queue_stats cohérentes).

```bash
CXXFLAGS="-include cstdint" cargo run --bin p10-s3-runner -- llama3.2:3b
# Expected: "P-α PASS", "P-β PASS", "P-γ PASS"
# Produit poc/p10_s3_report.json
```

---

### P10-S5 — Fairness et priorité

**Propriété :** Supervisor > Foreground dans la file d'inférence.
**Contrat :** A-priorité (superviseur passe avant foreground), A-E3 (aucune famine foreground), A-E1 (log QueueTrace cohérent).

```bash
CXXFLAGS="-include cstdint" cargo run --bin p10-s5-runner -- llama3.2:3b
# Expected: "A-priorité PASS", "A-E3 PASS", "A-E1 PASS"
```

---

## 5. Cas d'usage concrets (démo — nécessitent Ollama)

Ces runners n'ont pas d'exit code formel. Le critère est la présence des mots-clés
indiqués dans la sortie stderr.

### Tableau de référence

| ID | Runner | WASM requis | Propriété | Critère de validation | Lessons |
|----|--------|-------------|-----------|----------------------|---------|
| — | `chat-runner` | `multi_turn` | P1a basique | agent répond de façon cohérente | — |
| — | `pipeline-runner` | `multi_turn×2` | P3b basique | `action_id B` a `parent_ids` contenant `action_id A` | — |
| — | `rollback-runner` | `multi_turn` | P2 basique | `SchedulerRollback` dans la sortie | — |
| — | `long-task-runner` | `task_step` | P1a interruption | `[RESUMED FROM LOG]` dans la sortie | — |
| — | `evict-wake-runner` | `multi_turn` | ADR-0030 éviction | `Evicted → Dormant → Wake` dans la sortie | — |
| — | `memory-runner` | `task_step` | P1a mémoire longue | faits appris en session 1 rappelés en session 4 | — |
| — | `chain-runner` | `multi_turn×3` | P3b cross-agent | 3 agents avec causalité en chaîne | — |
| — | `parallel-runner` | `multi_turn×N` | parallélisme | N agents simultanés, timestamps entrelacés | — |
| — | `orchestrate-runner` | `orchestrator` + `multi_turn` | spawn dynamique | `Event(spawn)` puis nouvel agent dans le log | — |
| — | `supervisor-runner` | `llm_worker` + `llm_supervisor` | A3 basique | `ValidationRequest` + `ValidationResponse` dans le log | — |
| — | `support-runner` | `support_triage` + `task_step` | routing dynamique | `ANSWER` ou `ESCALADE` dans la sortie | — |
| — | `code-review-runner` | `code_reviewer` + `severity_judge` | pipeline multi-agent | verdict `CRITICAL/HIGH/LOW` dans la sortie | — |
| — | `consensus-runner` | `voter_agent×N` + `tally_secretary` | vote multi-agent | majorité calculée, dissidences préservées | — |
| — | `iterative-runner` | `task_step` + `critic_agent` | boucle critique | au moins 2 itérations draft→feedback dans le log | — |
| — | `incident-runner` | `task_step×3` + `incident_aggregator` | fan-out/fan-in | 3 spécialistes → agrégateur avec 3 parents | — |
| A | `decision-correction-runner` | `multi_turn` | **P2 concret** | `[ROLLBACK]` + recommandation changée après rollback | L105 (aucune adaptation) |
| B | `self-correct-runner` | `quality_writer` | **A2 concret** | `[SELF_CORRECTED]` dans ActionResult | L106 (WASM : static_mut_refs + non-ASCII) |
| D | `audit-query-runner` | `task_step×3` | **P3b DAG** | `18 nodes` (ou N nodes), BFS remonte à l'agent A | L107 (runner : entries_by_agent → query_by_agent_range) |
| E | `approval-runner` | `approval_agent` + `task_step` | **A3 concret** | `REJECTED -- PLAN BLOCKED BY SUPERVISOR` | L108 (runner : offset recherche ValidationRequest) |
| G | `capability-runner` | `data_accessor` | **P4 concret** | `3 WROTE / 3 DENIED` + `CapabilityDenied (0x14)` | L109 (runner : slash terminal dans grant_root) |
| I | `brainstorm-runner` | `brainstorm_synth` + `task_step×3` | **P3b fan-in** | `parent_ids (4)` avec MYTH/TECH/MOD dans audit | L110 (WASM : design no-barrier intentionnel) |
| J | `cross-session-runner` | `task_step` | **P1a cross-session** | Q1–Q4 corrects malgré RAM vide en session 2 | L111 + L112 (runner : messages consolidés) |
| K | `determinism-runner` | `echo` | **P5 déterminisme** | P-alpha/P-beta/P-gamma PASS — action_ids bit-à-bit identiques entre A et B | L113 (aucune adaptation) |
| L | `bounded-infer-runner` | `task_step×3` | **C1 borne inférence** | `max_waiting >= 1` + `total_rejected = 0` + 3 agents terminés | L114 (aucune adaptation) |
| M | `hierarchy-runner` | `hierarchy_synth` + `task_step×3` | **P3b hiérarchie** | `fan-in parent_ids (4)` + DAG depth >= 2 + cross-agent Mgr/Sec | L115 (WASM : nouveau hierarchy_synth.rs) |
| N | `watchdog-runner` | `task_step` (superviseur) | **ADR-0025 watchdog** | `AgentCrash(WatchdogTrap 0x03)` + rapport superviseur causal | L116 (aucune adaptation) |
| O | `observer-runner` | `monitor_agent` + `task_step×2` | **log bus partagé** | `parent_ids (3)` cross-agent A+B + supervision PASS | L117 (WASM : nouveau monitor_agent.rs) |
| P | `orphan-causality-runner` | `multi_turn` + `task_step` | **P2 orphelin** | `SchedulerRollback(0x0B)` dans log A + B.parent_ids → action_A_originale | L118 (runner : multi_turn requis pour rollback sur acteur vivant) |

---

### Commandes détaillées — use cases concrets

#### A — decision-correction-runner (P2)

```bash
CXXFLAGS="-include cstdint" cargo run --bin decision-correction-runner -- llama3.2:3b
# Validation:
#   ✓ "brief incomplet → NoSQL" dans plan initial
#   ✓ "[ROLLBACK]" après soumission du brief complet
#   ✓ "PostgreSQL" (ou équivalent ACID) dans la recommandation post-rollback
#   ✓ "Total entrées log après rollback > avant" (log append-only)
```

#### B — self-correct-runner (A2)

```bash
# Build WASM requis
cargo build --target wasm32-unknown-unknown -p agent-sdk --example quality_writer --release

CXXFLAGS="-include cstdint" cargo run --bin self-correct-runner -- llama3.2:3b
# Validation:
#   ✓ "[CONFIRMED]" (bon format dès le départ) OU "[SELF_CORRECTED]" (rollback appliqué)
#   ✓ Si rollback: "SelfRollback depth=1 target_seq=0" dans la sortie
#   ✓ Sortie finale commence par "ANNOUNCE:"
```

#### D — audit-query-runner (P3b DAG)

```bash
CXXFLAGS="-include cstdint" cargo run --bin audit-query-runner -- llama3.2:3b
# Validation:
#   ✓ "N nodes dans le DAG causal" (N ≥ 15)
#   ✓ Racine du DAG = agent A (le premier lancé)
#   ✓ Agent C a 2 parents dans parent_ids (action_B + action_A)
```

#### E — approval-runner (A3)

```bash
# Build WASM requis
cargo build --target wasm32-unknown-unknown -p agent-sdk \
  --example approval_agent --example task_step --release

CXXFLAGS="-include cstdint" cargo run --bin approval-runner -- llama3.2:3b
# Validation:
#   ✓ "Agent en AwaitingValidation (bloqué)" dans la sortie
#   ✓ Revieweur émet REJECT (plan contient DROP TABLE)
#   ✓ "REJECTED -- PLAN BLOCKED BY SUPERVISOR" dans résultat final
#   ✓ Plan provisoire + ValidationRequest + verdict = 3 types d'entrées dans le log
```

#### G — capability-runner (P4)

```bash
# Build WASM requis
cargo build --target wasm32-unknown-unknown -p agent-sdk --example data_accessor --release

CXXFLAGS="-include cstdint" cargo run --bin capability-runner -- llama3.2:3b
# Validation:
#   ✓ "3 WROTE / 3 DENIED" dans le résumé
#   ✓ "3 CapabilityDenied dans le log (émis par le runtime, sans code agent)"
#   ✓ resources WROTE: reports/quarterly/*, reports/annual/*, reports/draft/*
#   ✓ resources DENIED: confidential/*, admin/*
```

#### I — brainstorm-runner (P3b fan-in)

```bash
# Build WASM requis
cargo build --target wasm32-unknown-unknown -p agent-sdk \
  --example brainstorm_synth --example task_step --release

CXXFLAGS="-include cstdint" cargo run --bin brainstorm-runner -- llama3.2:3b
# Validation:
#   ✓ "parent_ids (4)" dans audit DAG
#   ✓ Les 3 action_ids [MYTH], [TECH], [MOD] présents dans parent_ids
#   ✓ "WINNER:" dans la décision finale
```

#### J — cross-session-runner (P1a cross-session)

```bash
CXXFLAGS="-include cstdint" cargo run --bin cross-session-runner -- llama3.2:3b
# Validation:
#   ✓ Session 1 : "8 entrées dans le log" (ou N > 0)
#   ✓ "RAM WASM de session 1 : DÉTRUITE"
#   ✓ "Réouverture store + log depuis /tmp/cross-session-..."
#   ✓ Session 2 : Q1 contient "Alice Moreau", Q2 contient "NovOS"
#   ✓ Q3 contient "15" (années d'expérience)
```

#### K — determinism-runner (P5 déterminisme)

```bash
# Prérequis : echo.wasm compilé en release
cargo build --target wasm32-unknown-unknown -p agent-sdk --example echo --release

CXXFLAGS="-include cstdint" cargo run --bin determinism-runner
# Pas de modèle Ollama requis (echo.wasm = introspect + barrier + emit, sans infer)
# Validation :
#   ✓ "P-alpha hash final identique  : PASS"
#   ✓ "P-beta  sequence action_ids   : PASS"
#   ✓ "P-gamma SHA-256 log digest    : PASS"
#   ✓ "PASS -- P5 determinisme (3 proprietes)"
#   ✓ Premiers action_ids instance A tous étiquetés "(= B)"
```

#### L — bounded-infer-runner (C1 borne inférence)

```bash
CXXFLAGS="-include cstdint" cargo run --bin bounded-infer-runner -- llama3.2:3b
# Validation :
#   ✓ "total_admitted  : 3"
#   ✓ "total_rejected  : 0"
#   ✓ "max_waiting obs : 1" (ou > 1, jamais 0)
#   ✓ "A-liveness (3 agents termines)         : PASS"
#   ✓ "A-no-drop  (total_rejected == 0)       : PASS"
#   ✓ "A-bounded  (max_waiting >= 1 observe)  : PASS"
```

#### M — hierarchy-runner (P3b hiérarchie + DAG)

```bash
# Prérequis : hierarchy_synth.wasm compilé en release
cargo build --target wasm32-unknown-unknown -p agent-sdk --example hierarchy_synth --release

CXXFLAGS="-include cstdint" cargo run --bin hierarchy-runner -- llama3.2:3b
# Validation :
#   ✓ Manager action_id visible dans la trace
#   ✓ "[SEC] ... cause=><action_id_manager>" (causalité niveau 1→2)
#   ✓ "[PERF] ... cause=><action_id_manager>" (causalité niveau 1→2)
#   ✓ "fan-in parent_ids (4 >= 3)     : PASS"
#   ✓ "DAG profondeur >= 2 niveaux    : PASS"
#   ✓ "cross-agent : Mgr + Sec dans le DAG    : PASS"
#   ✓ "PASS -- M : delegation hierarchique + audit DAG"
```

#### N -- watchdog-runner (ADR-0025)

Voir commandes en fin de section.

#### O -- observer-runner (log bus partage)

Voir commandes en fin de section.

#### P -- orphan-causality-runner (P2 orphelin)

Voir commandes en fin de section.

---

### Commandes — runners de démonstration basiques

```bash
# Agent conversationnel multi-tour
CXXFLAGS="-include cstdint" cargo run --bin chat-runner -- llama3.2:3b

# Pipeline A → B (causalité cross-agent)
CXXFLAGS="-include cstdint" cargo run --bin pipeline-runner -- llama3.2:3b

# Rollback de base
CXXFLAGS="-include cstdint" cargo run --bin rollback-runner -- llama3.2:3b

# Tâche longue avec interruption (P1a)
CXXFLAGS="-include cstdint" cargo run --bin long-task-runner -- llama3.2:3b

# Éviction + réveil (ADR-0030)
CXXFLAGS="-include cstdint" cargo run --bin evict-wake-runner -- llama3.2:3b

# Mémoire inter-sessions (P1a étendu)
CXXFLAGS="-include cstdint" cargo run --bin memory-runner -- llama3.2:3b

# Chaîne causale 3 agents
CXXFLAGS="-include cstdint" cargo run --bin chain-runner -- llama3.2:3b

# N agents en parallèle
CXXFLAGS="-include cstdint" cargo run --bin parallel-runner -- llama3.2:3b

# Spawn dynamique
CXXFLAGS="-include cstdint" cargo run --bin orchestrate-runner -- llama3.2:3b

# Validation superviseur (A3 basique)
CXXFLAGS="-include cstdint" cargo run --bin supervisor-runner -- llama3.2:3b

# Routing support (ANSWER vs ESCALADE)
CXXFLAGS="-include cstdint" cargo run --bin support-runner -- llama3.2:3b

# Revue de code multi-agent
CXXFLAGS="-include cstdint" cargo run --bin code-review-runner -- llama3.2:3b

# Consensus par vote majoritaire
CXXFLAGS="-include cstdint" cargo run --bin consensus-runner -- llama3.2:3b

# Boucle draft → critique itérative
CXXFLAGS="-include cstdint" cargo run --bin iterative-runner -- llama3.2:3b

# Triage d'incident (fan-out + fan-in)
CXXFLAGS="-include cstdint" cargo run --bin incident-runner -- llama3.2:3b
```

---

## 6. Scénarios use-cases S15–S35 (sans Ollama sauf mention)

Ces scénarios sont implémentés comme tests unitaires dans `poc/runtime/src/lib.rs`.
Ils couvrent les use cases UC-1/UC-2/…/UC-20 + UC-14 + UC-15 + UC-23 du catalogue `docs/use-cases-catalogue.md`.

### Lancer tous les scénarios S15–S35 en une commande

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- s15 s16 s17 s18 s19 s20 s21 s22 s23 s24 s25 s26 s27 s28 s29 s30 s31 s32 s33 s34 s35 --nocapture
```

### S15 — Crash machine concurrent + cache invalidé (UC-17 / P6)

**Propriété :** P6 sous concurrence + kill brutal + drop_caches.
**Requiert :** root ou sudo sans mot de passe.

```bash
cd poc
CXXFLAGS="-include cstdint" cargo build --release \
  -p os-poc-runtime --bin s15-writer --bin s15-verifier
sudo bash scenarios/S15-crash-machine-concurrent/run.sh
# Expected: "Verdict global : 5/5 pass"
```

### S16 — `agent_infer` annulé pendant `WaitingInference` (UC-10 / P2 × C1)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s16_infer_cancel_toctou --exact --nocapture
# Expected: ok — slot libéré, séquence 0x11/0x0E/0x0B/0x12 vérifiée
```

### S17 — Rollback + invalidation cap en cascade (UC-9 / P2 × P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s17_rollback_cap_cascade --exact --nocapture
# Expected: ok — C_A et C_B révoquées, C_root intacte
```

### S18 — `agent_add_cause` légitime : nœud de merge (UC-1 / P3c)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s18_add_cause_merge --exact --nocapture
# Expected: ok — parent_ids(C1) = [C0_id, A1_id], merge N=2
```

### S19 — Compensation journal orphelin (UC-12 / P6)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s19_compensation_orphelin --exact --nocapture
# Expected: ok — orphelin 0x11 détecté, ContentStore inchangé
```

### S20 — Propagation erreur cross-agent (UC-13 / P3/P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s20_propagation_crash_agent --exact --nocapture
# Expected: ok — parent_ids(B1) contient A1_id, canal A fermé proprement
```

### S21 — Délégation cap scope-prefix (UC-2 / P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s21_cap_delegation_scope_prefix --exact --nocapture
# Expected: ok — B accède à /data/sub, refusé hors-scope et hors-permission
```

### S22 — Session bornée (UC-3 / P3)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s22_session_bounded --exact --nocapture
# Expected: ok — SessionBoundary (0x0A), session_id=2, 1re action cite Checkpointed
```

### S23 — Canal validation chemin timeout (UC-4 / P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s23_validation_timeout --exact --nocapture
# Expected: ok — ValidationResponse verdict=Timeout dans le log
```

### S24 — Watchdog WASM budget (UC-5 / ADR-0025)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s24_watchdog_budget --exact --nocapture
# Expected: ok — AgentCrash (0x13) dans le log, elapsed < 4s
```

### S25 — Isolation de faute one_for_one (UC-6 / P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s25_restart_policy_one_for_one --exact --nocapture
# Expected: ok — A a AgentCrash, B n'a pas AgentCrash, B actif
```

### S26 — A1 introspection (UC-7 / A1)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s26_introspection_a1 --exact --nocapture
# Expected: ok — seq=0/1 dans les payloads Introspect, last_action correct
```

### S27 — Contrat emit (UC-8 / P6 nominal)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s27_emit_contract --exact --nocapture
# Expected: ok — hash_after dans le log == snapshot dans le store (I-CSR nominal)
```

### S28 — Self-rollback post-emit refusé (UC-11 / P2)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s28_self_rollback_post_emit_refused --exact --nocapture
# Expected: ok — snapshot inchangé, aucun SelfRollback (0x07) dans le log
```

### S29 — Révocation récursive profonde (UC-16 / P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s29_revoke_recursive_deep --exact --nocapture
# Expected: ok — 5 caps révoquées, check() = false pour toute la chaîne k=4
```

### S30 — WASM adversarial trap + isolation (UC-18 / P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s30_wasm_adversarial_trap_isolation --exact --nocapture
# Expected: ok — AgentCrash OOB (0x01), survivor actif, I-CSR intact
```

### S31 — Audit flood au-delà de la borne 32 (UC-19 / ADR-0051 §D2 / P4)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s31_audit_flood_beyond_bound_32 --exact --nocapture
# Expected: ok — limite documentée, "secret-33" masquée (F2 sentinel), P4-isolation intacte
```

### S32 — Forgerie causale B-light mono-tenant (UC-20 / ADR-0036 / P3)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s32_causal_forgery_b_light_monotenant --exact --nocapture
# Expected: ok — forgerie acceptée (B-light), parent_ids trompé, limite ADR-0036 documentée
```

### S33 — Anti-famine Batch→Foreground (UC-14 / ADR-0023 / P1b)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s33_anti_starvation_batch_promoted --exact --nocapture
# Expected: ok — Batch promu, total_promoted>=1, Batch servi avant FG2
```

### S34 — Déterminisme deux instances (UC-15 / ADR-0028 / P5)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s34_determinism_two_instances_same_hash --exact --nocapture
# Expected: ok — P-α last_snapshot≡, P-β action_ids≡, P-γ SHA-256≡ (LogicalClock)
```

### S35 — Tempête P2×P4×P6 (UC-23 / ADR-0001 / ordre d'arbitrage)

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s35_storm_p2_p4_p6_arbitrage --exact --nocapture
# Expected: ok — P4≻P2≻P6 tient, C_B révoquée cascade, InferenceCancelled, Compensation journal
```

---

## 7. Script de validation rapide (SEF déterministes)

Ce script vérifie les 7 tests SEF sans Ollama. Durée : ~5 min (SEF-5 est long).

```bash
#!/usr/bin/env bash
set -e
cd "$(dirname "$0")"   # doit être lancé depuis poc/

echo "=== Build runners SEF ==="
CXXFLAGS="-include cstdint" cargo build \
  --bin sef1-runner --bin sef2-runner --bin sef6-runner \
  --bin sef5-runner --bin sef12-runner --bin sef13-runner \
  --bin s11-runner \
  --bin sef4-victim --bin sef4-verify --features crash-injection

echo "=== SEF-1 persistance ==="
./target/debug/sef1-runner; echo "SEF-1: $?"

echo "=== SEF-2 rollback ==="
./target/debug/sef2-runner; echo "SEF-2: $?"

echo "=== SEF-6 déterminisme ==="
./target/debug/sef6-runner; echo "SEF-6: $?"

echo "=== SEF-12 rollback adversarial ==="
./target/debug/sef12-runner; echo "SEF-12: $?"

echo "=== SEF-13 traçabilité adversariale ==="
./target/debug/sef13-runner; echo "SEF-13: $?"

echo "=== S11 éviction/réveil ==="
./target/debug/s11-runner; echo "S11: $?"

echo "=== SEF-4 crash atomicity ==="
TMPDIR=$(mktemp -d)
./target/debug/sef4-victim "$TMPDIR" 100 50
./target/debug/sef4-verify "$TMPDIR"; echo "SEF-4: $?"

echo "=== SEF-5 traçabilité (long — ~2 min) ==="
./target/debug/sef5-runner; echo "SEF-5: $?"
echo "Nettoyage SEF-5..." && rm -rf /tmp/sef5-*

echo "=== DONE — tous les SEF doivent afficher exit code 0 ==="
```

---

## 7. Propriétés ↔ runners (index croisé)

| Propriété | Spec | Tests de qualification | Cas d'usage concrets |
|-----------|------|----------------------|----------------------|
| P1a — persistance log autoritaire | §P1a | SEF-1 | `long-task-runner`, `memory-runner`, J (`cross-session-runner`) |
| P1b — densité active (anti-famine) | §P1b | S5, S12, S33 | `evict-wake-runner` |
| P2 — rollback transactionnel | §P2 | SEF-2, SEF-12 | `rollback-runner`, A (`decision-correction-runner`), P (`orphan-causality-runner`) |
| P3a — traçabilité latence | §P3a | SEF-5 | — |
| P3b — DAG causal | §P3b | SEF-13 | D (`audit-query-runner`), I (`brainstorm-runner`), M (`hierarchy-runner`), O (`observer-runner`) · S32 (limite B-light) |
| P4 — capabilities runtime | §P4 | S21, S23, S25, S29, S30, S31 | G (`capability-runner`) |
| P5 — déterminisme | §P5 | SEF-6, S34 | K (`determinism-runner`) · S34 (LogicalClock replay) |
| P6 — atomicité crash | §P6 | SEF-4, ICSR, S15, S19, S27 | — |
| P2 × P4 — rollback + révoc. cascade | §P2/P4 | S17 | UC-23 (S35 — tempête) |
| P2 × P4 × P6 — ordre arbitrage ADR-0001 | §P2/P4/P6 | S35 | — |
| P2 × C1 — rollback + libération slot | §P2/C1 | S16 | — |
| P3c — causalité concurrente merge | §P3c | S18 | — |
| P3/P4 — propagation cross-agent | §P3/P4 | S20 | — |
| P3 — session bornée | §P3 | S22 | — |
| A1 — introspection agent | 02c §A1 | S26 | — |
| A2 — self-rollback post-emit refusé | §A2 | S28 | B (`self-correct-runner`) |
| A3 — validation superviseur | §A3 | S23 | `supervisor-runner`, E (`approval-runner`) |
| C1 — borne inférence | ADR-0022 | P10-S3, P10-S5 | L (`bounded-infer-runner`) |
| C2 — borne I/O scheduler | ADR-0030 | S10, S11, S12 | `evict-wake-runner` |
| ADR-0025 — watchdog | ADR-0025 | S24, S25 | N (`watchdog-runner`) |

---

## Notes

- **Nettoyage :** les runners créent des répertoires sous `/tmp/`. SEF-5 génère ~15 GB.
- **Modèle :** `llama3.2:3b` recommandé pour les tests rapides. Les runners acceptent
  n'importe quel modèle Ollama en 1er argument.
- **Logs de débogage :** ajouter `RUST_LOG=debug` devant la commande pour les traces
  détaillées du runtime.
- **Rebuild WASM après modification :** relancer `cargo build --target wasm32-unknown-unknown ...`
  — les binaires natifs ne recompilent pas les WASM automatiquement.
