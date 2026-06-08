# S8 — Déterminisme de transition d'état (SEF-6 / P5)

## Objectif

Vérifier la propriété **P5 — Déterminisme de transition d'état**
(`spec/02-properties.md §P5`) par observation systémique :

> Deux instances du système, initialisées avec un état d'agent identique (hash
> vérifié) et alimentées avec une séquence enregistrée de 1 000 messages dans le
> même ordre, doivent produire des séquences de messages émis identiques et des
> hash d'état finaux identiques.

La propriété combinée à la primitive horloge substituable (ADR-0028, S6) exerce,
dans un seul scénario d'intégration, l'invariant central de P5 : **mêmes inputs,
même horloge, même séquence d'effets observables, même état final**.

## Pourquoi l'horloge substituable est nécessaire

Sans substitution, deux instances séparées dans le temps lisent
`SystemTime::now()` à des instants différents. Ces timestamps sont insérés dans :

- `SnapshotHeader.ts_us` → `snapshot_id` (hash SHA-256 du header) → `last_snapshot`
  → `hash_before` / `hash_after` des `LogEntry` suivants.
- `LogEntry.ts_ms` → `action_id` (hash SHA-256 de l'entrée sérialisée).
- `EmitEnvelope.ts_us` → payload MessagePack → `LogEntry.emit_payload` → `action_id`.

La conséquence : deux runs identiques produisent des chaînes content-addressed
**bytewise différentes** sur tous les artefacts. SEF-6 est non-vérifiable et P5
devient une vœux pieu.

La primitive `Clock` (`poc/runtime/src/clock.rs`) résout ce problème :

- En production, `SystemClock` retourne `SystemTime::now()`.
- En mode replay/SEF-6, `LogicalClock(start)` est un compteur monotone
  déterministe : chaque appel `now_ms()` ou `now_us()` retourne la valeur
  courante puis incrémente. Deux instances initialisées avec la même valeur
  `start` et soumises à la même séquence d'inputs produisent la même série de
  timestamps.

L'API publique préexistante reste inchangée (rétro-compatibilité totale) ; un
nouveau constructeur `ActorInstance::new_precompiled_with_clock` est ajouté pour
les usages de replay/test.

Voir ADR-0028 pour la justification complète.

## Propriétés vérifiées par run

| Code | Propriété                                                  | Mesure |
|------|------------------------------------------------------------|--------|
| P-α  | `last_snapshot(A) == last_snapshot(B)`                     | Hash final du ContentStore. Falsifié si une source non-déterministe entre dans la chaîne content-addressed (timestamp non-substitué, ordre des actions différent, etc.). |
| P-β  | `action_ids(A) == action_ids(B)` bytewise                  | Séquence complète des `action_id` (clés RocksDB du CausalLog) ordonnés par l'index `agent_ts`. Falsifié si un seul `LogEntry` diffère (ts_ms, parent_ids, ou emit_payload). |
| P-γ  | `SHA256(action_ids(A)) == SHA256(action_ids(B))`           | Hash agrégé : vue compacte de P-β. Utile pour les rapports/audits. |

Le verdict pass = les 3 propriétés vraies. Si P-β échoue, le binaire reporte
l'index du premier point de divergence et les deux `action_id` correspondants
— signature de la position dans la séquence où la non-substitution se manifeste.

## Configuration

| Paramètre              | Valeur |
|------------------------|--------|
| Agent WASM             | `AGENT_WAT` (commit_barrier + emit ActionResult) |
| Payload de chaque message | `b"sef6-XXXXXXXX"` (numéroté, identique entre instances) |
| N (actions)            | 1 000  |
| Horloge                | `LogicalClock(start=1700000000000)` (identique A/B) |
| Répétitions            | 5      |
| Agent ID               | varie par run (octet bas porte r) — A et B partagent le même par run |
| Runtime                | Tokio current_thread |

## Périmètre — ce qui n'est PAS testé

- **agent_infer** : la primitive d'inférence introduit deux sources non
  contrôlées par `Clock` — le contenu de la réponse (backend stochastique) et
  `Instant::now().elapsed()` (chronométrage d'inférence inscrit dans le payload
  `InferenceResponse`). Couvrir SEF-6 sur un agent qui appelle `agent_infer`
  requiert un backend mocké déterministe et un record-and-replay sur la durée
  mesurée — explicitement hors scope (cf. spec §P5 « résultats d'inférence
  stochastique » comme source S6 nécessitant enregistrement).
- **Déterminisme d'exécution complet** (timings, ordonnancement Tokio, latences)
  : exclu par spec/05-non-goals.md §3.3. Seul l'**état observable** est garanti
  identique, pas l'historique d'exécution.
- **Scheduler::rollback et journal de compensation** : `emit_compensation_open` /
  `emit_compensation_close` (`scheduler.rs`) écrivent dans le log avec
  `agent_id = SCHEDULER_AGENT_ID`. Ces entrées ne sont pas reproductibles entre
  runs (`SystemTime::now()` direct). Hors périmètre SEF-6 : S8 n'invoque jamais
  `Scheduler::rollback`.

## Critère d'acceptation

Pour chaque répétition r ∈ 1..=K_RUNS :
- `sef6-runner` doit se terminer avec exit code 0 (les 3 propriétés vraies).

Total : 5 runs. Verdict global = pass si 5/5 passent.

## Exécution

```bash
cd poc
bash scenarios/S8-determinism/run.sh
```

Sortie attendue (release) :

```
[S8] Compilation sef6-runner (release)...
[S8] run 1/5: pass
[S8] run 2/5: pass
...
[S8] Verdict global : 5/5 pass
```

Exit code 0 si 5/5 pass.

## Sortie

`scenarios/S8-determinism/report.json` après run :

```json
{
  "timestamp": "...",
  "scenario": "S8-determinism",
  "property": "P5",
  "sef": "SEF-6",
  "n_actions": 1000,
  "clock_start": 1700000000000,
  "k_runs": 5,
  "passed": 5,
  "total": 5,
  "verdict": "pass"
}
```

## Sources de non-déterminisme identifiées et corrigées

Audit complet 2026-05-18 (avant ADR-0028) — 24 call-sites `SystemTime::now()`
dans `poc/runtime/src/`. Catégorisation :

| Catégorie | Fichier | Affecte hash état ? | Affecte action_id ? | Traitement |
|-----------|---------|---------------------|---------------------|------------|
| commit_barrier — SnapshotHeader.ts_us, PendingCommit.ts_ms | actor.rs:901 | OUI | OUI | Substitué par `state.clock.now_us()` / `now_ms()` |
| emit — EmitEnvelope.ts_us | actor.rs:1244 | NON | OUI | Substitué par `state.clock.now_us()` |
| log_lifecycle_event — LogEntry.ts_ms, EmitEnvelope.ts_us | actor.rs:524 | NON | OUI | Substitué |
| log_agent_crash — LogEntry.ts_ms | actor.rs:572 | NON | OUI (hors SEF-6) | Substitué |
| record_validation_response — LogEntry.ts_ms | actor.rs:618 | NON | OUI | Substitué |
| log_session_boundary — LogEntry.ts_ms | actor.rs:648 | NON | OUI | Substitué |
| agent_self_rollback — LogEntry.ts_ms | actor.rs:1080 | NON | OUI | Substitué |
| agent_request_validation — LogEntry.ts_ms | actor.rs:1157 | NON | OUI | Substitué |
| Message::Rollback handler — LogEntry.ts_ms | actor.rs:1778 | NON | OUI | Substitué |
| session bounds check — comparaison wall-clock | actor.rs:1545 | indirect | indirect | Substitué |
| agent_infer phase 1/3 — LogEntry.ts_ms | actor.rs:1331/1347 | NON | OUI (hors SEF-6) | Substitué |
| scheduler.rs emit_compensation_* | scheduler.rs:222/229 | NON | OUI (SCHEDULER_AGENT_ID) | **Non substitué** — hors périmètre SEF-6 |
| inference/queue.rs Instant::now | queue.rs | NON | NON (timing local) | Inchangé — non-déterminisme d'exécution |
| agent_infer Instant::now (duration_ms) | actor.rs:1342 | NON | OUI via InferenceResponse payload | **Non substitué** — wall-clock irréductible, agent_infer hors SEF-6 |

11 call-sites sur 14 substitués via `state.clock`. Les 3 restants sont
explicitement hors scope SEF-6 et documentés dans le périmètre ci-dessus.

## Références

- `spec/02-properties.md §P5` — propriété de déterminisme de transition d'état.
- `spec/02b-substrate_requirements.md §S6` — source d'horloge isolable et substituable.
- `spec/05-non-goals.md §3.3` — déterminisme d'exécution complet exclu.
- ADR-0028 — Horloge substituable Clock (introduit la primitive).
- `poc/runtime/src/clock.rs` — implémentation `SystemClock` et `LogicalClock`.
- `poc/runtime/src/bin/sef6_runner.rs` — binaire de ce scénario.
