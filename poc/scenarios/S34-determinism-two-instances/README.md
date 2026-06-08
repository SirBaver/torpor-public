# S34 — Déterminisme deux instances (UC-15 / ADR-0028 / P5)

**Régime :** R2 (P5 — déterminisme de transition d'état, `LogicalClock`)
**Substrat :** Linux. Non transférable à seL4 (D7).

---

## Ce qui est testé

**P5 (déterminisme de transition)** : deux instances distinctes (ContentStore + CausalLog
séparés sur disque), même agent_id, même module WASM (`AGENT_WAT`), même `LogicalClock`
initialisé à la même valeur (`CLOCK_START = 1_700_000_000_000`), même séquence de N=100
messages → même état final, même journal causal.

Trois propriétés mesurées :

| Id | Propriété | Oracle |
|----|-----------|--------|
| P-α | `last_snapshot` bit-à-bit identique | `snap_a == snap_b` |
| P-β | Séquence ordonnée des `action_id` identique | `ids_a == ids_b` |
| P-γ | SHA-256 de la concaténation des `action_id` identique | `hash_a == hash_b` |

---

## Mécanisme — pourquoi LogicalClock est indispensable

Chaque `action_id` est dérivé du contenu de l'entrée log, qui inclut `ts_ms` et `ts_us`
(timestamps). Avec `SystemClock`, les timestamps diffèrent entre les deux runs → `action_id`
diverge dès la première entrée.

Avec `LogicalClock(start)` : chaque appel à `now_ms()` / `now_us()` retourne la valeur
courante et incrémente de 1. Si la séquence d'appels est identique (garanti par le canal
`mpsc` séquentiel + `current_thread`), les deux instances produisent les mêmes timestamps
→ les mêmes `action_id` → le même état final.

```
Instance A : clock=LogicalClock(1_700_000_000_000)
             message[0] → commit_barrier → ts_ms=1_700_000_000_000 → action_id_A0
             message[1] → commit_barrier → ts_ms=1_700_000_000_001 → action_id_A1
             ...

Instance B : clock=LogicalClock(1_700_000_000_000)  ← même valeur initiale
             message[0] → commit_barrier → ts_ms=1_700_000_000_000 → action_id_B0
             → action_id_B0 == action_id_A0  ✓
```

---

## Limite documentée (UC-15)

> **Reproductibilité sémantique seulement** si un LLM est impliqué.

Sans backend d'inférence, `AGENT_WAT` ne fait qu'`emit(type=1, payload)` — purement
déterministe. Avec un LLM réel (sampling stochastique), les sorties du modèle divergeront
entre instances même sous `LogicalClock`. UC-15 est la propriété de la **couche runtime**
(timestamps, action_ids, snapshots) — pas de l'inférence. Le backend mocké déterministe
(`SleepyBackend`) est hors-scope de ce test.

---

## Oracles

```
Oracle P-α : actor_a.last_snapshot() == actor_b.last_snapshot()
Oracle P-β : log_a.entries_by_agent(AGENT_ID) == log_b.entries_by_agent(AGENT_ID)
Oracle P-γ : SHA-256(concat(ids_a)) == SHA-256(concat(ids_b))
```

---

## Ce qui n'est PAS testé

| Hors-scope | Raison |
|------------|--------|
| N=1000 (sef6-runner) | Test d'intégration externe (`sef6_runner` binaire) |
| SystemClock (non-déterminisme) | Sans LogicalClock, P5 n'est pas vérifiable — c'est le point de départ |
| Backend LLM mocké déterministe | Hors périmètre UC-15 ; nécessiterait `SleepyBackend` déterministe |
| seL4 | D7 — substrate requirement |

---

## Comment relancer

```bash
cd poc
CXXFLAGS="-include cstdint" cargo test -p os-poc-runtime --release \
  -- tests::s34_determinism_two_instances_same_hash --exact --nocapture
```

---

## Références

- **ADR-0028** — Horloge substituable (P5 — déterminisme de transition d'état).
- `poc/runtime/src/clock.rs` `LogicalClock` — implémentation.
- `poc/runtime/src/actor.rs` `new_precompiled_with_clock` (l.1126) — constructeur avec horloge.
- `poc/runtime/src/bin/sef6_runner.rs` — pendant N=1000 (binaire d'intégration).
- `poc/runtime/src/lib.rs::tests::s34_determinism_two_instances_same_hash` — test.
