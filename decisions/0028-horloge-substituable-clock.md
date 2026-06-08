# ADR-0028 — Horloge substituable Clock pour SEF-6 (S6 / P5)

**Date :** 2026-05-18
**Statut :** Acceptée

---

## Contexte

La propriété **P5 — Déterminisme de transition d'état** (`spec/02-properties.md
§P5`) est une garantie conditionnelle : elle tient si et seulement si les
exigences substrat **S1** (isolation mémoire) et **S6** (horloge et entropie
substituables, `spec/02b-substrate_requirements.md §S6`) sont satisfaites
conjointement.

L'audit du runtime au 2026-05-18 (avant cet ADR) compte **24 call-sites
`SystemTime::now()`** répartis dans `actor.rs`, `scheduler.rs` et le pool
d'inférence. Les conséquences :

- `commit_barrier` insère `now_us` dans `SnapshotHeader.ts_us` qui rentre dans
  `snapshot_id = SHA256(bincode(header))`. Le `snapshot_id` devient
  `last_snapshot`, qui est lui-même hashé dans `LogEntry.hash_before` /
  `hash_after` du commit suivant. **Une variation de 1 ms entre deux runs produit
  une chaîne content-addressed bytewise différente sur tous les artefacts.**
- `emit` insère `now_us` dans `EmitEnvelope.ts_us` qui est sérialisé en
  MessagePack et stocké dans `LogEntry.emit_payload`. Le `LogEntry.action_id` =
  `SHA256(bincode(LogEntry))` capture donc indirectement ce timestamp.
- 7 autres call-sites (`log_lifecycle_event`, `log_session_boundary`,
  `agent_self_rollback`, `agent_request_validation`, `Message::Rollback` handler,
  etc.) insèrent un `now_ms` dans `LogEntry.ts_ms` ou un `now_us` dans
  `EmitEnvelope.ts_us` — tous deux hashés.

**Conséquence :** SEF-6 est non-vérifiable en l'état. Le coût annoncé par
`spec/02-properties.md §P5 Coût connu` — « interface plus stricte plutôt que
coûteux en performance » — n'avait pas été payé. P5 restait un vœu.

L'introduction d'une primitive horloge substituable est la condition
préalable à toute vérification SEF-6. Sans elle, P5 doit être marqué
« non-applicable au substrat retenu » dans la grille de
`spec/02b-substrate_requirements.md`.

## Décision

Nous introduisons le trait `crate::clock::Clock` dans `os-poc-runtime`, avec
deux implémentations : `SystemClock` (mode production, adossée à
`SystemTime::now()`) et `LogicalClock` (mode replay/SEF-6, compteur monotone
déterministe). L'horloge est stockée comme champ `clock: Arc<dyn Clock>` dans
`AgentState`. Tous les call-sites qui inscrivent un timestamp dans une structure
hashée (`SnapshotHeader.ts_us`, `LogEntry.ts_ms`, `EmitEnvelope.ts_us`) passent
par `state.clock.now_*()`.

Les constructeurs existants (`new_precompiled`, `new_precompiled_with_caps`,
etc.) délèguent à `build_instance_inner_with_profile_and_clock` avec
`Arc::new(SystemClock)` par défaut — **rétro-compatibilité totale, aucune
signature publique préexistante n'est modifiée**. Un nouveau constructeur
`ActorInstance::new_precompiled_with_clock` est ajouté pour le mode replay.

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A. Thread-local mutable global** (`thread_local!{ static CLOCK }`) | Aucun changement de signature ; impl trivial | Sémantique implicite ; conflit avec Tokio multi-thread ; deux acteurs colocalisés ne peuvent pas avoir d'horloges distinctes | Rejetée : casse la composition multi-acteur (cf. SEF-6 mono-process avec 2 instances) |
| **B. Variable d'environnement** `CLOCK_FIXED_START_MS` | Mécanisme connu (CRIU, rr) | Global, pas de granularité par-acteur ; difficile à tester | Rejetée : même défaut que (A) au niveau process |
| **C. WASI clocks injectables** (cf. `wasi:clocks/wall-clock`) | Standard, déjà mentionné dans `spec/03-state-of-the-art.md §WASI Preview 2` | Notre runtime utilise des host functions custom (commit_barrier, emit, agent_*), pas WASI clocks ; les timestamps sont lus côté **host**, pas côté WASM | Rejetée : ne couvre pas les call-sites host (la majorité) |
| **D. Refactor complet — `Clock` injectée à toutes les fonctions** | Pureté maximale ; aucun état caché | Très haut churn : ~30 signatures à modifier ; casse tous les call-sites de test | Rejetée : coût démesuré pour le bénéfice du moment |
| **E. `Clock` dans `AgentState` via `Arc<dyn Clock>`** | Couvre tous les call-sites host ; granularité par-acteur ; rétro-compatible via constructeurs `_with_clock` ; coût runtime négligeable (1 vtable call par host fn) | Ajoute un champ à `AgentState` (impact `Send` à vérifier — OK : `Arc<dyn Clock + Send + Sync>` est Send) | **Retenue** |

## Conséquences

**Positives :**

- SEF-6 devient vérifiable. Le binaire `sef6-runner` (`poc/runtime/src/bin/sef6_runner.rs`)
  produit 3 propriétés (P-α, P-β, P-γ) sur les hash finaux et la séquence
  d'action_ids. Le scénario S8 (`poc/scenarios/S8-determinism/`) valide 5/5 runs.
- La primitive est réutilisable pour tout test futur de reproductibilité (replay
  d'un bug, fuzzing déterministe style Antithesis/Foundation DB).
- Le coût annoncé dans `spec/02-properties.md §P5 Coût connu` est explicitement payé :
  l'interface des host functions est plus stricte (passage par `state.clock`).

**Négatives / coûts acceptés :**

- Toute future host function qui produit un timestamp inséré dans une structure
  hashée doit utiliser `state.clock.now_*()` au lieu de `SystemTime::now()`. Une
  régression sur cette discipline rendrait SEF-6 instable. Mitigations :
  (a) un commentaire au-dessus de chaque host fn dans `actor.rs` rappelle
  l'invariant ; (b) S8 est exécuté en CI à chaque PR touchant `actor.rs`.
- `Arc<dyn Clock>` ajoute un overhead négligeable mais non nul : un appel virtuel
  par `now_ms()` / `now_us()`. Sur le chemin chaud `commit_barrier + emit`, ça
  représente 4 appels par action — sous l'overhead de la sérialisation bincode
  + écriture RocksDB (~10 µs mesuré). Non mesurable empiriquement.

**Périmètre — call-sites NON substitués (hors SEF-6) :**

- `scheduler.rs::emit_compensation_open` / `emit_compensation_close` :
  `agent_id = SCHEDULER_AGENT_ID` ([0xFF;16]). Ces entrées ne font pas partie
  de la « séquence de messages émis par un agent » au sens P5. Hors périmètre.
- `inference/queue.rs::admission_instant` : `Instant::now()` utilisé uniquement
  pour le scheduling interne du pool (priorité, promotion, traçabilité QueueTrace).
  Pas inscrit dans une structure hashée. Hors périmètre P5 §3.3
  (déterminisme d'exécution, exclu).
- `actor.rs::1342 — Instant::now()` dans `agent_infer` : capture `duration_ms`
  inscrit dans le payload `InferenceResponse`. **Hors périmètre SEF-6** :
  `agent_infer` introduit deux sources non-déterministes (contenu de la réponse
  backend, durée mesurée). SEF-6 sur un agent qui appelle `agent_infer` requiert
  un backend mocké déterministe + record-and-replay sur la durée — non implémenté.

**Neutres / à surveiller :**

- Si un substrat futur (ex. seL4) impose une horloge kernel-mediated, l'interface
  `Clock` reste valide : il suffit de fournir une impl adossée à la primitive
  substrat. Pas de remise en question architecturale.
- La granularité par-acteur (un `Arc<dyn Clock>` par `AgentState`) permet à
  terme de modéliser des dérives d'horloge entre acteurs distincts — utile pour
  les tests de protocoles distribués qui supposent que les acteurs n'ont pas
  une horloge commune.

## Validation expérimentale

`scenarios/S8-determinism/run.sh` — N=1000 messages, K=5 runs, AGENT_WAT.
Verdict 5/5 pass à 2026-05-18. Voir `scenarios/S8-determinism/report.json`.

Smoke test antérieur (N=100) : pass. Test N=1000 isolé : pass.

## Références

- `spec/02-properties.md §P5` — déterminisme de transition d'état (propriété vérifiée).
- `spec/02b-substrate_requirements.md §S6` — exigence substrat satisfaite par cet ADR.
- `spec/05-non-goals.md §3.3` — déterminisme d'exécution complet exclu (justifie le périmètre).
- `spec/03-state-of-the-art.md §WASI Preview 2` — l'injection de clocks WASI est citée comme
  démonstration de S6 ; cet ADR applique le même principe au niveau host function.
- [Hewitt 1973] *Actor model* — séquentialité + communication comme unique observabilité.
- [Reynolds 2023] Antithesis / [Kulkarni 2023] Foundation DB simulation — référence
  d'architecture pour la substitution complète des sources non-déterministes.
- `poc/runtime/src/clock.rs` — implémentation.
- `poc/runtime/src/bin/sef6_runner.rs` — binaire de vérification SEF-6.
- ADRs liées :
  - ADR-0001 — Ordre de priorité (P5 ≻ P1 ; P5 reste vérifiable même si P1 dégradé).
  - ADR-0010 — Contrat d'emit (l'`EmitEnvelope.ts_us` substitué est sérialisé selon ADR-0010 §2).

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
