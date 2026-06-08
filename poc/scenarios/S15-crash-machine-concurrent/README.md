# S15 — Crash machine concurrent (UC-17 / ADR-0050 D4 / ADR-0027 §D3)

**Régime :** R1 (P6 — atomicité crash, propriété d'effet, actif indépendamment de la topologie d'inférence)
**Substrat :** Linux. Verdict non transférable à seL4 (D7 / ADR-0050).

---

## Ce qui est testé

La propriété **P6 — Atomicité crash** dans le régime jamais couvert par S6 :
**N agents en écriture concurrente + kill machine + cache invalidé.**

S6 (SEF-4) valide P6 sous crash processus (SIGKILL) avec page-cache intact.
S15 ferme le trou structurel (ADR-0050 D4) en ajoutant :

1. **Concurrence** — N threads écrivent en parallèle sur le même store/log RocksDB.
   Les écritures de différents agents s'entrelacent dans la memtable et le WAL.
2. **Kill brutal** — `process::exit(1)` alors que K agents ont leurs commits en vol
   (entre `put_snapshot` et `append`, fenêtre ~1–50 µs). Le buffer WAL applicatif
   de RocksDB est perdu ; seules les écritures déjà flushed dans le page cache OS
   survivent.
3. **Invalidation de cache** — `sync ; echo 3 > /proc/sys/vm/drop_caches` après la
   coupure. Toutes les dirty pages sont d'abord flushées sur disque (`sync`), puis
   le page cache est vidé. La réouverture lit depuis le disque froid.

**Oracle (valid-prefix)** : P6 est violée si, après recovery, pour au moins un agent,
au moins l'un des cas suivants est observé :
- **(a) Écart (gap)** — un commit acké N est visible dans le log mais un commit acké
  M < N ne l'est pas (trou dans la séquence).
- **(b) Commit partiel** — un commit visible dans le log a son `snapshot_hash` absent
  du ContentStore (référence pendante — `SnapshotMissing`), ou son data-block absent
  (`DataBlockMissing`).
- **(c) `parent_id` pendant** — un commit visible dans le log référence un `parent_id`
  qui n'existe pas dans le log reconstruit.

**Résultat attendu :** P6 tient. Le log post-recovery est un préfixe valide de la
séquence ackée par agent (pertes en queue acceptées ; pertes internes ou états partiels
= violation).

---

## Garde-fous de recevabilité

| Code | Règle |
|------|-------|
| **L32** | Un `kill -9` sans invalidation de cache ne teste pas la durabilité (durabilité fantôme via page-cache). Ce scénario exige `sync + drop_caches` — documenté dans `run.sh`. |
| **D7** | Verdict Linux uniquement. Non transférable à seL4. |
| **F1** | Ce scénario n'utilise pas l'InferenceBackend (pas de `agent_infer`). P6 est une propriété d'effet — recevable avec ou sans inférence locale. |

---

## Acteur(s)

| Nom | Source | Rôle |
|-----|--------|------|
| `s15-writer` | `poc/runtime/src/bin/s15_writer.rs` | Spawne N threads d'écriture concurrente, collecte le témoin, se tue via `process::exit(1)` au seuil |
| `s15-verifier` | `poc/runtime/src/bin/s15_verifier.rs` | Rouvre store + log depuis disque froid, vérifie l'oracle valid-prefix par agent |

Pas d'agent WASM. Les écritures sont synthétiques (appels directs à
`ContentStore` + `CausalLog`, identique au harness ICSR).

---

## Protocole

```
  Thread-1 (Agent-1)          Thread-2 (Agent-2)   ...  Thread-N (Agent-N)
  ──────────────────          ──────────────────         ──────────────────
  loop {                      loop {                     loop {
    put_block(data)               put_block(data)            put_block(data)
    put_snapshot(header)          put_snapshot(header)       put_snapshot(header)
    log.append(entry) ──ack─→    log.append(entry) ──ack─→  log.append(entry) ──ack─→
    acked.push(commit)            acked.push(commit)         acked.push(commit)
    total_acked += 1              total_acked += 1           total_acked += 1
  }                           }                          }
         │                          │                          │
         └──────────────────────────┴──────────────────────────┘
                                    │
                         total_acked ≥ KILL_THRESHOLD
                                    │
                    Main thread: set kill_flag
                    sleep(2ms)    ← fenêtre adversariale : certains threads
                                     sont entre put_snapshot et append
                    collect per-thread witnesses
                    save witness.json
                    process::exit(1)  ← tous les threads tués
                                    │
              Shell: sync + echo 3 > /proc/sys/vm/drop_caches
                                    │
              s15-verifier (disque froid):
                pour chaque agent:
                  1. chercher acked commits dans le log
                  2. vérifier préfixe valide (pas de gap)
                  3. vérifier I-CSR pour le préfixe visible
                  4. vérifier parent_ids
```

---

## Configuration par défaut

| Paramètre | Valeur | Signification |
|-----------|--------|---------------|
| `N_AGENTS` | 4 | Threads concurrent writers |
| `COMMITS_PER_AGENT` | 25 | Commits planifiés par agent (100 total) |
| `KILL_THRESHOLD` | 40 | Commits ackés avant kill (40 % du total) |
| `BLOCK_SIZE` | 64 B | Taille du data-block synthétique |
| `K_RUNS` | 5 | Répétitions du run complet |

---

## Critères d'acceptation

Pour chaque run parmi K_RUNS :

1. `s15-writer` se termine avec exit code 1 (kill contrôlé).
2. `s15-verifier` se termine avec exit code 0 (P6 satisfait).

Falsification : exit code 1 du verifier, avec au moins une violation de type
`Gap`, `SnapshotMissing`, `DataBlockMissing`, ou `ParentIdMissing` dans le rapport.

Verdict global = **PASS** si K_RUNS/K_RUNS passent.

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| Power-loss réel (coupure secteur) | Requiert hardware qualifié avec PLP (ADR-0027 §D4). `process::exit(1) + sync + drop_caches` ne simule pas la perte des dirty pages avant flush. |
| Durabilité sous power-loss (`sync=true` sur chemin chaud) | Phase 7+ (ADR-0027 §D3/D4). Déclencheur : T5-bis sur hardware qualifié + fsync ≤ 5 ms. |
| Cross-agent parent_ids (DAG causal multi-agents) | Chaque agent a sa propre chaîne indépendante dans ce harness. La causalité cross-agent est testée par UC-1/SEF-7, pas S15. |
| Recovery avec scheduler actif | Ce harness n'utilise pas le runtime Tokio complet (pas d'acteurs WASM). Testé par S4/S6. |
| seL4 | D7 — non transférable. |

---

## Comment relancer

```bash
cd poc
sudo bash scenarios/S15-crash-machine-concurrent/run.sh
```

Ou avec paramètres :
```bash
sudo bash scenarios/S15-crash-machine-concurrent/run.sh \
  [N_AGENTS] [COMMITS_PER_AGENT] [KILL_THRESHOLD] [K_RUNS]
```

Compilation préalable (si les binaires ne sont pas à jour) :
```bash
cd poc
CXXFLAGS="-include cstdint" cargo build --release \
  -p os-poc-runtime --bin s15-writer --bin s15-verifier
```

Sortie attendue :
```
[S15] Run 1/5
[S15]   writer: exit(1) après 40 acks (4 agents, 25 commits/agent planifiés)
[S15]   drop_caches OK
[S15]   verifier: agent-1 préfixe=10 acks, 0 gaps, 0 violations I-CSR
[S15]   verifier: agent-2 préfixe=9 acks, 0 gaps, 0 violations I-CSR
[S15]   verifier: agent-3 préfixe=11 acks, 0 gaps, 0 violations I-CSR
[S15]   verifier: agent-4 préfixe=10 acks, 0 gaps, 0 violations I-CSR
[S15]   Run 1/5: pass
...
[S15] Verdict global : 5/5 pass
```

---

## Prérequis

- Rust toolchain stable + `wasm32-unknown-unknown` (ADR-0020 D1)
- `CXXFLAGS="-include cstdint"` (GCC récent, librocksdb-sys)
- **root ou sudo sans mot de passe** pour `echo 3 > /proc/sys/vm/drop_caches`
- Linux uniquement (D7)

---

## Références

- **ADR-0050 D4** — Axe 3 : crash concurrent à invalidation de cache. Oracle valid-prefix.
- **ADR-0027 §D3** — Régime power-loss vs SIGKILL. RocksDB WAL buffer applicatif.
- **ADR-0024** — Pattern CrashPoint, journal de compensation (référence pour S6).
- **ADR-0021** — Convention scénarios S<N>.
- **S6-crash-atomicity** — Précurseur : crash niveau process, page-cache intact.
- **ICSR-drop-caches** — Précurseur : crash + drop_caches, agent unique séquentiel.
- `poc/runtime/src/durability.rs` — `verify_p6_concurrent()`, `ConcurrentWitness`.
- `lab/LESSONS.md` L32 — Page-cache fantôme.
- `spec/02-properties.md §P6` — Atomicité crash.
