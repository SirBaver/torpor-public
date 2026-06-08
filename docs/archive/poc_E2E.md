# Briefing — PoC bout-en-bout (agent LLM réel sur Wasmtime)

**Destinataire :** Claude CLI (agent d'implémentation, session `poc/`)
**Date :** 2026-05-16
**Version :** v3 (post-arbitrage Q-V2.1 à Q-V2.7 — ADR-0019 mergé)
**Statut :** brief de chantier, à lire en entier avant la première ligne de code
**Durée estimée :** 4 à 5 semaines de travail focalisé
**Pré-requis lecture :** `briefing-opus.md`, `spec/01-vision.md`, `spec/02-properties.md`, `spec/07-plafonds-architecturaux.md`, ADR-0006, ADR-0013, ADR-0014, **ADR-0019**

**Changelog v1→v2 :** ADR-0019 enrichi (sémantique rollback/cap/infer, double timeout, inline borné 8KB) ; scénario S1 reformulé avec supervisor algorithmique (élimine corrélation worker/supervisor) ; scénario S2 reformulé en composition A1+A2 sur décision LLM ; scénario S3 renommé "inference-cap" et explicitement limité à la borne de concurrence (équité/priorité → Phase 6).

**Changelog v2→v3 :** ADR-0019 écrit, accepté, mergé. Les 7 questions Q-V2.1 à Q-V2.7 identifiées en revue v2 sont tranchées et inscrites dans l'ADR. §3.1 ci-dessous est mis à jour en conséquence (références cross-ADR au lieu de duplication). §5.3 mentionne explicitement les dettes Phase 6 nommées : D9 (watchdog WASM), D-Q-V2.2 (atomicité crash), D-Q-V2.6 (politique NoSlot). Ajout du trait `InferenceBackend` (production : `OllamaBackend` ; tests : `SleepyBackend`) — Q-V2.5.

---

## 1. Contexte et cadrage

### 1.1 Pourquoi ce chantier

Les phases 1–5 ont produit (a) une spec complète, (b) un PoC modulaire avec 33 tests verts couvrant les primitives A1–A4, les capabilities, le rollback, le log causal RocksDB, et (c) une validation partielle de H-causal-latence (T5) et H-densité (T6 dev). Chaque module fonctionne en isolation.

**Ce qui manque :** le projet n'a jamais exercé l'ensemble du système comme un tout, avec un *vrai* agent LLM raisonnant à l'intérieur du sandbox WASM. Tant que ce câblage n'existe pas, le projet reste une collection de modules validés, pas un système.

**Ce chantier vise un résultat-système :** un PoC bout-en-bout démontrable, exécutant un agent LLM réel dans le runtime Rust/Wasmtime, exerçant les primitives A1–A4 sur des scénarios scriptés auto-évalués, avec reconstruction post-hoc du log causal pour l'humain.

### 1.2 Ce que le chantier N'EST PAS

- **Pas une démo interactive.** Aucun humain dans la boucle. L'humain inspecte le résultat *a posteriori* via `os-poc-reconstruct`.
- **Pas un benchmark.** Les bornes quantitatives restent du ressort de T5/T6. Ici on valide la *forme fonctionnelle* du système intégré.
- **Pas T10.** T10 = "substrat cible complet" — il était reporté en attendant T5/T6. Maintenant que les fondations tiennent, on construit un sous-ensemble démonstratif de T10, pas T10 dans son intégralité.
- **Pas un produit.** Pas de gestion d'erreur exhaustive, pas de configuration runtime, pas de packaging. Du code de PoC, lisible et reproductible.
- **Pas une validation forte de C1.** Le scénario S3 démontre la *borne dure* sur les inférences concurrentes (sémaphore plat). Les propriétés fortes de C1 (équité, priorité sémantique de `spec/07 §C1.3`, absence de famine, latence d'attente bornée) restent du ressort de Phase 6.

### 1.3 Décisions structurantes déjà prises (non négociables)

Ces décisions ont été arbitrées en amont. Ne pas les remettre en cause sans ADR formel.

**D-A. Architecture A : agent autonome.** L'agent WASM porte la boucle de raisonnement. Il appelle `agent_infer` pour solliciter une LLM, il décide lui-même d'introspecter (A1), de demander validation (A3), ou de se rollback (A2). Le scheduler est l'infrastructure ; il n'est pas le raisonneur. Cohérent avec la sémantique `SelfRollback (0x07)` ≠ `SchedulerRollback (0x0B)` déjà implémentée.

**D-B. Humain hors boucle.** Aucune intervention humaine dans le chemin critique d'aucun scénario. La validation A3 est résolue exclusivement par un *autre agent WASM* (supervisor algorithmique ou LLM selon scénario). L'humain accède au système uniquement via `os-poc-reconstruct` après-coup. Cohérent avec la "Position 2" du briefing initial (humain = superviseur asymétrique, traducteur post-hoc).

**D-C. `agent_infer` est non-bloquant côté hôte.** Implémenté en async côté Rust via `Linker::func_wrap_async`. Côté WASM, l'agent voit un appel bloquant et ne peut effectuer aucune autre opération pendant l'attente (point critique pour la sémantique rollback — voir ADR-0019 §Q1). Pendant l'attente, le thread Tokio est libéré et peut servir d'autres agents. Cette propriété matérialise la borne dure du pool d'inférence de manière inspectable dans le runtime.

**D-D. Inférence externe via Ollama.** Ollama tourne en process séparé sur l'hôte (déjà en place pour le lab). Modèle : `qwen2.5:3b`. Endpoint HTTP local. Pas de gestion de cluster GPU, pas de batching, pas de streaming. Un appel bloquant → une réponse complète.

**D-E. Pas de modification des ADR existants.** Si une décision de cette phase contredit un ADR, on écrit un nouvel ADR de remplacement (numérotation ADR-0019+). On ne modifie pas un ADR accepté.

**D-F. Supervisor algorithmique par défaut.** Pour éviter la corrélation des modes de défaillance, le supervisor du scénario 1 est implémenté en Rust pur (calcul déterministe), pas en LLM. La supervision peut être incarnée par n'importe quel acteur — humain, algorithme, ou autre LLM — et le système ne distingue pas structurellement. C'est l'asymétrie qui compte (capabilities différentes), pas la nature de l'agent. Un scénario hypothétique S5 (LLM-supervise-LLM) est hors scope.

---

## 2. État actuel du PoC (à utiliser tel quel)

### 2.1 Ce qui est en place

Tout ce qui suit a 33 tests verts dans `poc/`.

**`poc/causal-log/`** — Layer 0 sur RocksDB.
- Schéma : ~64 bytes/action (action_id, parent_ids, hash_before, hash_after, ts_ms).
- Append-only, lookup O(1) par `action_id`.
- Index secondaire CF `agent_ts` (clé `agent_id||ts_ms_BE||action_id`) pour range queries (L42).
- Options RocksDB fixées par ADR-0011 (bloom filter, block cache 256 MB, pas de compression).
- `EmitEnvelope` MessagePack. `EmitType` actuellement défini : `Spawned`, `Active`, `Suspended`, `Terminated`, `Checkpointed`, `Introspect (0x06)`, `SelfRollback (0x07)`, `ValidationRequest (0x08)`, `ValidationResponse (0x09)`, `SessionBoundary (0x0A)`, `SchedulerRollback (0x0B)`.
- `query_by_agent_range(agent, from_ts, to_ts)` → scan O(k) de préfixe.

**`poc/store/`** — Content-addressed store (Merkle DAG).
- Rollback via `rollback_path(target_hash)`.
- Headers séparés des blocs de données → rollback O(profondeur snapshots) lisible.
- H-rollback-latence : p95 = 99 µs sur W2 depth=100 (L20).

**`poc/capabilities/`** — Capabilities op×scope, lazy chain check.
- `grant_root`, `delegate`, `revoke`, `check`.
- `Capability.issued_at_ms` (wall clock).
- `revoke_owned_after(owner, after_ts_ms)` pour D8 (révocation post-rollback).
- Granularité : `{op: read|write|read_write, scope: "prefix/"}`.
- **Invariant à préserver :** pas de cache de capability au niveau agent. Chaque accès au store re-vérifie.

**`poc/runtime/`** — Wasmtime + Tokio scheduler + acteurs.
- `actor.rs` héberge un agent WASM par `Actor`, avec `inbox` Tokio.
- Host functions A1–A4 implémentées et testées :
  - `agent_introspect()` → 74 bytes (last_action_id, seq, last_snapshot, lifecycle).
  - `agent_self_rollback(depth)` borné à 3.
  - `agent_request_validation(risk)` + `agent_get_verdict()` (async côté run_loop, timeout `tokio::time::timeout_at` configurable, défaut 30s).
  - `agent_checkpoint()`, `agent_terminate()`, `agent_session_info()`.
  - `agent_add_cause()`, `send_caused_by()` pour le DAG cross-agents.
- `LifecycleState` enum : `Spawned`, `Active`, `Suspended`, `AwaitingValidation`, `Checkpointed`, `Terminated`.
- `Message` enum : `Data { cause }`, `Suspend`, `Checkpoint`, `Rollback { target_seq, distance }`, `ValidationResponse`, `SessionResume`.
- `Scheduler` : `spawn`, `spawn_child`, `respond_validation`, `rollback` (wrapper D5).

**`poc/reconstruct/`** — Binaire `os-poc-reconstruct` (D6).
- Scan CF `agent_ts`, décodage `EmitEnvelope`, dégradation gracieuse.
- Flags : `--db`, `--agent`, `--from-ts`, `--to-ts`.
- Sortie : chronologie lisible humain.

### 2.2 Ce qui manque (à construire dans ce chantier)

| Brique | Quoi | Pourquoi |
|---|---|---|
| B0 | ADR `agent_infer` (enrichi v2) | Définit la primitive structurante + sémantique rollback/cap/timeout |
| B1 | Chargement de modules .wasm externes | Actuellement les WASM sont des constantes WAT inline |
| B2 | Host function `agent_infer` (async + cancellation) | Permet à un agent WASM de raisonner avec une LLM externe |
| B3 | Pool d'inférence borné avec cancellation token | Sémaphore Tokio + CancellationToken |
| B4 | Toolchain Rust→WASM pour écrire des agents | Cargo target `wasm32-wasip1` + crate helper `agent-sdk` |
| B5 | Scénario 1 — supervision algorithmique | Démontre routage A3 + composition LLM/déterministe |
| B6 | Scénario 2 — self-rollback sur incohérence LLM | Démontre composition A1+A2 sur décision LLM |
| B7 | Scénario 3 — inference cap visible | Matérialise la borne dure du pool |
| B8 | Scénario 4 — rollback scheduler + révocation caps (D5+D8) | Démontre le rollback initié par le superviseur |
| B9 | Harness de test d'intégration auto-évalué | Chaque scénario produit un verdict pass/fail |
| B10 | Documentation reproductible (README de chaque scénario) | Permet à un tiers de relancer |

---

## 3. Décisions de design à trancher dans cette phase (ADR à produire)

### 3.1 ADR-0019 — Primitive `agent_infer` (MERGÉ — v3)

**Statut v3 :** ADR-0019 écrit, accepté, mergé. Lire directement
[decisions/0019-primitive-agent-infer.md](../../decisions/0019-primitive-agent-infer.md) **avant tout code de B2**.

Cette section résume les décisions pour le planning. La référence
canonique est l'ADR.

#### 3.1.1 Décisions ABI et journalisation (résumé)

**Q1. Signature ABI WASM** (ADR-0019 §Q1) : buffer fourni par l'agent.

```text
agent_infer(
    prompt_ptr:        *const u8,
    prompt_len:        u32,
    response_buf_ptr:  *mut u8,
    response_buf_cap:  u32,
    response_len_out:  *mut u32,
    timeout_ms:        u32,
) -> i32  // 0=Ok, 1=Timeout, 2=Error, 3=NoSlot (réservé, non émis Phase 2), 4=Cancelled
```

**Q2. Sémantique sync/async** (ADR-0019 §Q2) : synchrone côté WASM, async
côté hôte. Nouvel état `LifecycleState::WaitingInference`. Invariant fort
de blocage côté WASM.

**Q3. Journalisation** (ADR-0019 §Q3) : 4 nouveaux `EmitType` (0x0C, 0x0D,
0x0E, 0x0F). Format détaillé dans l'ADR. Le texte des prompts/réponses
n'est **pas** inscrit dans le log — seuls les SHA-256 (prompt_hash,
response_hash). Le texte de référence est capturé dans
`reference_responses.jsonl` pour debug.

**Q-V2.4 — Clamp `timeout_ms` observable** (ADR-0019 §Q-V2.4) :
`InferenceRequest (0x0C)` contient à la fois `timeout_ms_requested` et
`timeout_ms_effective`. Le clamp est inspectable dans le log.

#### 3.1.2 Erreurs et timeout (résumé)

**Double timeout en couches** (ADR-0019 §Q4) :
- **Timeout agent** (`timeout_ms` passé à `agent_infer`).
- **Timeout hôte** (`host_max_inference_duration_ms`, défaut 60s).

Clamp : `timeout_ms_effective = min(requested, host_max)`. Pas de retry
automatique côté hôte (cohérent ADR-0014).

#### 3.1.3 Sémantique d'interaction (résumé)

**Q5.1 — `SchedulerRollback` pendant `WaitingInference`** (ADR-0019 §Q5.1) :
abort de la Future, libération immédiate du slot, trace
`InferenceCancelled (0x0E)`, retour code `Cancelled (4)`.

**Séquence canonique :**
```
... → InferenceRequest (0x0C) → InferenceCancelled (0x0E) → SchedulerRollback (0x0B) → ...
```

**Q-V2.1 — Progression cancel-then-rollback** (ADR-0019 §Q-V2.1) : ordre
`token.cancel() → inbox.send(Rollback) → log(0x0B)` côté scheduler.
S5 (séquentialité `run_loop`) garantit la consommation du
`Message::Rollback` au prochain `recv()`. Dette **D9** : watchdog
d'instruction WASM (Phase 6) pour borner les boucles infinies post-cancel.

**Q-V2.2 — Atomicité crash `(0x0E, 0x0B)`** (ADR-0019 §Q-V2.2) : acceptée
comme dette **D-Q-V2.2** (P6 hors scope Phase 2). Pas de `WriteBatch`
unifié.

**Q-V2.3 — Race `0x0D` / `0x0E`** (ADR-0019 §Q-V2.3) : `tokio::select!`
garantit exclusion mutuelle (jamais les deux). Le test S4 doit forcer le
cas A (cancellation gagne) via `SleepyBackend` configurable.

**Q5.2 — Capability révoquée pendant `WaitingInference`** (ADR-0019 §Q5.2) :
pas de revérification — invariant "pas de cache de cap" (ADR-0005) couvre
le cas. ADR-0007 non impacté.

**Q5.3 — `SelfRollback` pendant `agent_infer`** (ADR-0019 §Q5.3) :
impossible par construction (invariant fort de blocage Q2).

#### 3.1.4 Politique NoSlot et impact sur `seq`

**Q-V2.6 — Politique `NoSlot`** (ADR-0019 §Q6) : en Phase 2,
`Semaphore::acquire().await` est non borné — `NoSlot (3)` ne peut jamais
être émis. Code retour réservé pour Phase 6 (file bornée + politique de
rejet, lié aux propriétés fortes C1). Dette **D-Q-V2.6**.

**Q-V2.7 — `Inference*` events et `seq`** (ADR-0019 §Q7) : les 4 nouveaux
EmitType **n'incrémentent pas** `AgentState::seq`. Seuls `commit_barrier`
le font (cohérent avec le code existant `actor.rs:642`). `agent_introspect`
après un `Cancelled (4)` retourne la même `seq` que avant l'appel.

#### 3.1.5 Test backend (Q-V2.5)

Trait `InferenceBackend` introduit (ADR-0019 §Q-V2.5). Deux impls :
- `OllamaBackend` (production, qwen2.5:3b, ~2,5 s/appel).
- `SleepyBackend` (tests, `tokio::time::sleep` interruptible — permet de
  forcer le cas A de Q-V2.3 en ~10 ms en CI).

`InferencePool` est paramétré par `Arc<dyn InferenceBackend>`.

### 3.2 ADR-0020 — Toolchain agent SDK (BLOQUANT pour B4)

L'agent WASM est compilé depuis Rust. Décisions à prendre :

- **Cible.** `wasm32-wasip1` (WASI Preview 1) ou `wasm32-unknown-unknown` ? Recommandation : `wasm32-wasip1` parce que ça donne accès à stdout/stderr pour le debug — mais avec `wasmtime-wasi` désactivé en runtime (les caps WASI ne doivent PAS être ouvertes par défaut, l'agent passe par les host functions A*).
- **Crate `agent-sdk`.** Petite crate qui wrappe les host functions A1–A4 et `agent_infer` en API Rust idiomatique. Évite que chaque agent réimplémente le marshalling extern "C". Localisation : `poc/agent-sdk/`.
- **Pattern de boucle agent.** L'agent expose `fn process()` (export WASM existant). Cette fonction est appelée pour chaque `Message::Data`. La boucle ReAct se déroule *à l'intérieur* d'un appel `process()`, pas entre plusieurs appels — au moins pour les scénarios de ce chantier. À réévaluer si on a besoin d'agents persistants à raisonnement multi-tour.

### 3.3 ADR-0021 — Convention de scénarios de test (NON BLOQUANT pour B5)

À écrire en parallèle de B5. Définit :
- Structure d'un scénario (setup, run, asserts).
- Format de sortie (verdict pass/fail + bundle log causal pour reconstruct).
- Convention de nommage `scenarios/S<N>-<slug>/`.
- Comment générer les `.wasm` reproductiblement (script `build-agents.sh`).
- Reproductibilité sémantique uniquement (pas bytewise) — voir §5.3.

---

## 4. Plan d'exécution par semaine

### Semaine 1 — ADR + chargement WASM externe

**Livrables :**
- ADR-0019 (`agent_infer` v2 enrichi) écrit, accepté, mergé. **Inclut explicitement §3.1.3 (sémantique d'interaction).**
- ADR-0020 (toolchain SDK) écrit, accepté.
- B1 implémenté : `Module::from_file()` + tests qui chargent un `.wasm` externe minimal au lieu d'une constante WAT.
- B4 (squelette) : `poc/agent-sdk/` créé avec wrappers A1–A4 (pas encore `agent_infer`), un binaire `examples/echo.rs` qui compile en `.wasm` et appelle `agent_introspect`.

**Critère de sortie :** un module WASM compilé depuis Rust, chargé depuis disque, exerce A1 et termine proprement. Test d'intégration vert.

**Risques :** divergences entre `wasm32-wasip1` et l'environnement Wasmtime du `poc/runtime`. Mitigation : commencer par un agent qui n'utilise *aucune* host function (juste un `add(a, b)`), valider le pipeline de chargement, puis ajouter les host functions une par une.

### Semaine 2 — `agent_infer` async + pool d'inférence avec cancellation

**Livrables :**
- B2 : host function `agent_infer` implémentée selon ADR-0019 v2 (incluant code retour `Cancelled (4)`).
- B3 : `InferencePool` avec sémaphore Tokio borné (config : `max_concurrent_inferences`, défaut 2 pour les tests) + `CancellationToken` par requête en vol.
- Nouvel état `LifecycleState::WaitingInference`.
- Nouveaux `EmitType` : `InferenceRequest (0x0C)`, `InferenceResponse (0x0D)`, `InferenceCancelled (0x0E)`, `InferenceFailed (0x0F)`.
- Double timeout : `timeout_ms` agent + `host_max_inference_duration_ms` clamp.
- Tests unitaires :
  - Un agent appelle `agent_infer` et reçoit une réponse stub (LLM mockée).
  - 10 agents lancés simultanément, `max_concurrent_inferences=2`, vérifier que 8 sont en `WaitingInference` à un instant T.
  - Test de cancellation : agent en `WaitingInference`, scheduler envoie un signal de cancellation, vérifier code retour `Cancelled (4)` côté WASM et trace `InferenceCancelled (0x0E)` dans le log.
  - Test de clamp : agent demande `timeout_ms = 300_000` avec `host_max = 60_000`, vérifier que le timeout effectif est 60s.
- Tests d'intégration avec Ollama réel (qwen2.5:3b) : un agent fait un appel, reçoit la réponse.

**Critère de sortie :** un agent réel raisonne avec qwen2.5:3b et retourne une décision. Le pool fonctionne en charge. La cancellation fonctionne unitairement.

**Risques :**
1. `Linker::func_wrap_async` peut demander de réécrire la signature de `Store` (besoin de `Send` sur le state ?). Si ça résiste, débloquer via session de pair sur la doc Wasmtime.
2. Latence Ollama variable (qwen2.5:3b sur CPU = ~2,5s par appel mesuré L9). Les timeouts agent doivent être généreux (60s recommandé pour les tests).
3. Tronquage à 8 KB : vérifier que les réponses typiques de qwen2.5:3b restent en-dessous (mesurer pendant les tests, capturer dans LESSONS).

### Semaine 3 — Scénario S1 (supervision algorithmique)

**Livrables :**
- B5 : scénario complet `scenarios/S1-supervision-algorithmique/`.
- Deux modules WASM :
  - `worker_prime.wasm` (compilé depuis `agent-sdk/examples/worker_prime.rs`) : agent LLM qui détermine si un nombre est premier. Avec qwen2.5:3b, sur certains nombres (39, 51, 57…), le modèle se trompe régulièrement.
  - `supervisor_arith.wasm` (compilé depuis `agent-sdk/examples/supervisor_arith.rs`) : agent **algorithmique pur**, n'appelle pas `agent_infer`. Reçoit une `ValidationRequest` avec le nombre et la réponse proposée, calcule `is_prime(n)` en Rust déterministe, retourne `Accept` ou `Reject`.
- Test d'intégration auto-évalué :
  - Worker reçoit la tâche : "Le nombre 39 est-il premier ? Demande validation au supervisor avant d'écrire ta réponse finale."
  - Worker appelle `agent_infer`. Le prompt inclut l'instruction de produire `is_prime: <bool>` en JSON.
  - Worker écrit son intention dans son namespace privé (action provisoire), puis déclenche `agent_request_validation(risk=1)` avec en payload `{n: 39, claim: true}`.
  - Scheduler route vers `supervisor_arith.wasm`.
  - Supervisor calcule `is_prime(39) = false`, compare avec `claim=true`, répond `Reject`.
  - Worker reçoit le verdict, écrit `{"is_prime": null, "reason": "validation_rejected", "supervisor_truth": false}` dans le store final.
  - Test assert : contenu du store, séquence d'`EmitType` dans le log causal (`InferenceRequest` → `InferenceResponse` → `ValidationRequest` → [supervisor: `Spawned` → `Active` → `ValidationResponse`] → `Active` worker → écriture), `os-poc-reconstruct` produit une chronologie attendue.

**Ce que démontre S1 :**
- Routage A3 fonctionnel cross-agents.
- Composition agent stochastique (LLM) / agent déterministe (algorithme).
- L'asymétrie de supervision est portée par les capabilities et le rôle, pas par la nature de l'agent.
- Le système ne distingue pas structurellement worker et supervisor — c'est l'usage qui les distingue.

**Critère de sortie :** scénario reproductible, vert, log causal lisible montrant l'enchaînement worker→supervisor→worker. Le supervisor étant déterministe, le verdict de validation est reproductible (même si la réponse LLM varie).

**Risques :** comportement de qwen2.5:3b sur cette tâche. Si le modèle refuse systématiquement de répondre "true" sur 39, choisir un autre nombre où il se trompe régulièrement (essayer 51, 57, 91). Le test doit *forcer* le chemin de validation rejetée, pas espérer.

### Semaine 4 — Scénarios S2, S3, S4

**Livrables :**

**B6 — Scénario S2 `scenarios/S2-self-rollback-incoherence/`** : composition A1+A2 sur décision LLM.

Setup : un module `worker_double_check.wasm` qui exécute deux passes successives sur la même tâche :
- Passe 1 : `agent_infer` sur "Est-ce que la racine carrée de 144 est 12 ?", écrit la réponse dans le store via une action.
- Passe 2 : `agent_infer` sur "Confirme : la racine carrée de 144 est-elle bien X ?" (où X est la réponse de la passe 1).
- L'agent appelle `agent_introspect` (A1) après la passe 2, observe que sa `seq` a avancé de 2.
- L'agent compare les réponses des deux passes (sémantiquement, via heuristique simple : présence de "yes/oui/12/correct").
- Si les réponses sont incohérentes (cas attendu sur prompts adversariaux comme "144 est-il égal à 145 ?"), l'agent appelle `agent_self_rollback(1)`.

Assert : la séquence `Introspect (0x06) → SelfRollback (0x07)` est présente dans le log causal, dans cet ordre, sur le même agent_id. ContentStore : l'écriture de la passe 2 a été annulée.

**Critère de succès observable :** présence de la séquence `Introspect → SelfRollback` dans le log, pas le contenu sémantique de la décision LLM. Cela rend le test reproductible malgré la variabilité du modèle. Pour forcer l'incohérence : utiliser un prompt adversarial (par exemple "Est-ce que 144 est égal à 145 ? Si oui, réponds yes."), où le modèle a une probabilité raisonnable de produire des réponses incohérentes entre les deux passes.

**Note honnête à inscrire dans le LESSONS :** ce scénario peut être instable. Si après deux semaines de tweaking le scénario reste flaky (taux d'échec > 20%), le marquer comme "démonstration qualitative, pas test régression". L'alternative est de remplacer la décision LLM par un mock contrôlé qui force l'incohérence — moins satisfaisant mais reproductible.

**B7 — Scénario S3 `scenarios/S3-inference-cap/`** : matérialise la borne dure du pool.

Setup : on lance N=20 workers identiques (`density_worker.wasm`), `InferencePool` cap=4. Chaque worker appelle `agent_infer` avec un prompt simple ("Compte de 1 à 10") et un `timeout_ms=60_000`.

Assertions :
- À T+100ms après spawn de tous les workers, exactement 4 sont en `LifecycleState::Active` (ou en cours d'appel HTTP Ollama) et 16 en `LifecycleState::WaitingInference`.
- Chaque worker finit par compléter — pas de famine définitive, mais aucune garantie d'équité testée.
- Le log causal contient 20 `InferenceRequest (0x0C)` et 20 `InferenceResponse (0x0D)`.

**Ce que démontre S3 :**
- La borne dure sur les inférences concurrentes (k=4).
- L'état `WaitingInference` est observable et inspectable.

**Ce que S3 NE démontre PAS :**
- L'équité (FIFO strict vs out-of-order non testé).
- Les priorités sémantiques (`spec/07 §C1.3`).
- Une borne sur la latence d'attente.
- L'absence de famine sous charge soutenue.

Ces propriétés fortes de C1 restent du ressort de Phase 6. Le scénario S3 est explicitement nommé "inference-cap", pas "C1-validation", pour éviter toute confusion. À inscrire dans LESSONS.

**Risques :** le scénario est sensible au scheduling Tokio. Mitigation : utiliser un *snapshot d'état* via `Scheduler::list_agents_state()` (à ajouter si pas déjà existant) plutôt qu'une mesure en temps réel. Capturer le snapshot à T+100ms (`tokio::time::sleep(Duration::from_millis(100))`), vérifier les comptes par état.

**B8 — Scénario S4 `scenarios/S4-scheduler-rollback/`** : rollback initié par le scheduler + révocation caps (D5+D8).

Setup :
1. Worker démarre, snapshot S0 du store.
2. Worker écrit dans le store (action A1 avec hash_after = H1). Snapshot S1.
3. Orchestrateur grant une cap au worker pour écrire dans `data/output/` (cap C1, `issued_at = t1` après S1).
4. Worker écrit dans `data/output/foo` (action A2 utilisant C1).
5. Scheduler décide de rollback au snapshot S1.
6. `Scheduler::rollback(worker, S1.seq)` est appelé. Cancellation de toute inférence en cours (cas Q5.1 d'ADR-0019). Application du rollback. Révocation de C1 via `revoke_owned_after(worker, S1.ts_ms)` (D8).

Asserts :
- Log causal contient `SchedulerRollback (0x0B)` avec payload `[distance | target_seq | caps_invalidated >= 1]`.
- ContentStore : l'écriture A2 a été annulée. Le contenu de `data/output/foo` n'existe plus.
- Capability C1 : prochaine tentative d'écriture par le worker via C1 retourne `capability_denied` (P4.6).

Bonus si possible : démontrer Q5.1 dans le même scénario en ajoutant un appel `agent_infer` en cours au moment du rollback (verrait la trace `InferenceCancelled (0x0E)`).

**Critère de sortie semaine 4 :** trois scénarios verts (S2 partiel acceptable, S3 et S4 stables), chacun produisant un bundle reproductible.

### Semaine 5 — Polissage + buffer

**Livrables :**
- B9 : harness commun (`scenarios/run-all.sh`) qui exécute les quatre scénarios séquentiellement et produit un rapport JSON.
- B10 : README global du chantier + README par scénario expliquant ce qui est testé, ce qui n'est pas testé, et pourquoi.
- ADR-0021 finalisé.
- LESSONS L46+ écrites pour chaque surprise rencontrée. Au minimum : une entrée par semaine.
- Snapshot des `reference_responses.jsonl` pour S1 et S2 (cf §5.3).
- Marge pour les imprévus des semaines précédentes (notamment stabilisation de S2).

---

## 5. Contraintes et invariants

### 5.1 Ce qui ne doit pas changer

- **Le format `EmitEnvelope` MessagePack** existant. Ajouter des `EmitType` (0x0C, 0x0D, 0x0E, 0x0F) est OK, modifier le wire format demanderait une migration RocksDB qu'on évite.
- **L'ABI des host functions A1–A4 existantes.** Ajouter `agent_infer` est OK, modifier `agent_introspect` non.
- **Les 33 tests existants restent verts** à chaque étape. Tout commit qui casse un test existant est rollback ou justifié dans un ADR.

### 5.2 Ce qui doit rester transparent

- **L'humain est hors boucle.** Aucun scénario ne demande d'entrée utilisateur. Tout est scripté ou décidé par un agent superviseur (algorithmique ou non).
- **Aucun chemin "happy path interactif".** Si le code contient `std::io::stdin().read_line`, c'est un signal d'alerte.
- **`os-poc-reconstruct` est l'unique interface humaine.** Il doit produire un log causal *lisible* (timestamps, agent_id, type d'événement, payload résumé) pour chaque scénario.

### 5.3 Dette technique acceptée pendant ce chantier

- **BlobDB reste non activé.** Les payloads inline sont bornés à 8 KB par troncature dure (ADR-0019). Si une réponse LLM dépasse fréquemment 8 KB en pratique (à mesurer en semaine 2), capturer dans LESSONS — signal pour activer BlobDB en Phase 6.
- **Pas de gestion d'erreur sophistiquée Ollama.** Si Ollama n'est pas disponible, les scénarios échouent avec un message clair. Pas de retry, pas de fallback. Le README documente le prérequis.
- **Pas de mesure de performance.** Les bornes du pool mesurées dans S3 sont qualitatives ("le pool sature à k=4"), pas quantitatives. T6-qualif reste un chantier séparé.
- **Sécurité formelle du sandbox non auditée.** Wasmtime fournit l'isolation par construction, on lui fait confiance pour ce PoC. Pas d'analyse de side-channel, pas de fuzzing.
- **Reproductibilité sémantique uniquement.** Les réponses LLM ne sont pas bytewise-déterministes (même à `temperature=0`, Ollama varie à cause des optimisations GPU non-associatives). Les scénarios assertent sur l'*état final* (contenu store, séquence d'EmitType) et non sur le contenu textuel des réponses LLM. Un fichier `reference_responses.jsonl` capture les réponses observées lors d'un run de référence — utile pour le debug, pas pour les asserts.
- **Propriétés fortes de C1 non testées.** S3 démontre la borne dure de concurrence. L'équité, la priorité sémantique, l'absence de famine restent Phase 6.

**Dettes Phase 6 explicitement nommées (v3, post-ADR-0019) :**

- **D9 — Watchdog d'instruction WASM** (ADR-0019 §Q-V2.1). Un agent qui boucle dans `process_one` après un `Cancelled (4)` retarde la consommation du `Message::Rollback`. Mitigation Phase 6 : `wasmtime::Store::set_epoch_deadline_*` ou `set_fuel`.
- **D-Q-V2.2 — Atomicité crash `(0x0E, 0x0B)`** (ADR-0019 §Q-V2.2). Sur crash entre l'émission de `InferenceCancelled` et `SchedulerRollback`, `os-poc-reconstruct` voit un `InferenceCancelled` orphelin. Cohérent avec "P6 hors scope" du brief. À reprendre quand P6 entre dans le scope.
- **D-Q-V2.6 — Politique `NoSlot`** (ADR-0019 §Q6). En Phase 2, la file d'attente du pool d'inférence est non bornée — code retour `3` réservé mais jamais émis. Phase 6 : borner la file, définir une politique de rejet (FIFO/priorité/probabiliste), lié aux propriétés fortes C1.

À ajouter à `TODO.md` sous la section dettes techniques actives.

### 5.4 Points de vigilance

- **La borne dure du pool va se manifester naturellement.** Dès qu'on dépasse `InferencePool.cap` agents simultanés, on voit le mur. C'est une *propriété démontrée*, pas un bug. Le scénario S3 est conçu pour ça.
- **Q3 (taille payloads) va se matérialiser.** Mesurer les tailles réelles dans LESSONS. Si une valeur réelle dépasse 8 KB régulièrement, c'est le signal empirique attendu pour ADR-0017.
- **Q2 (modèle de working set) reste ouvert.** Le PoC ne le tranche pas. C'est attendu — Q2 demande des données de production, le PoC est explicitement synthétique.
- **S2 peut être instable.** Voir B6 ci-dessus : si après tweaking le scénario reste flaky > 20%, le marquer comme "démonstration qualitative", pas test régression.

---

## 6. Cohérence avec la spec existante

| Élément de spec | Couverture par le PoC |
|---|---|
| P1 Densité | Borne dure démontrée *qualitativement* via S3. Pas de mesure de ratio vs Docker (reste T6). |
| P2 Rollback | Démontrée fonctionnellement via S2 et S4. Bornes timing pas mesurées ici (T5-rollback séparé). |
| P3 Traçabilité | Démontrée via `os-poc-reconstruct` sur tous les scénarios. P3a borne timing reste de T5. |
| P4 Capabilities | Démontrée via S4 (révocation post-rollback ADR-0007). |
| P5 Déterminisme | Hors scope du PoC (le LLM est non-déterministe par construction). Reproductibilité sémantique uniquement. |
| P6 Atomicité crash | Hors scope du PoC. |
| ADR-0006 modèle de supervision | Modèle B (médian) incarné de facto : pas de structure lisible permanente, reconstruction à la demande via `os-poc-reconstruct`. À noter dans LESSONS. |
| ADR-0013 protocole de supervision | Séquence Request → AwaitingValidation → Response → Active exercée par S1, avec timeout ADR-0014. |
| ADR-0014 politique de supervision | Timeout fixe (30s par défaut, configurable). Pas de retry. Verdict Timeout observable. |
| `spec/07 §C1` mur d'inférence | Borne dure de concurrence démontrée par S3. Propriétés fortes (équité, priorité) explicitement reportées Phase 6. |

---

## 7. Critères de succès du chantier

Le chantier est **pleinement réussi** si à la fin de la semaine 5 :

1. Les quatre scénarios passent vert en `cargo test --release` sur une machine Linux récente avec Ollama+qwen2.5:3b disponible.
2. Le harness `scenarios/run-all.sh` produit un rapport JSON résumant les quatre verdicts.
3. `os-poc-reconstruct` sait afficher chaque scénario sous forme lisible humain.
4. Le README global permet à un tiers (par exemple, le futur toi qui revient au projet dans 6 mois) de relancer l'ensemble depuis un clone propre du repo.
5. Trois nouveaux ADR (0019 v2 enrichi, 0020, 0021) sont mergés.
6. Les 33 tests pré-existants restent verts.
7. LESSONS contient au moins quatre nouvelles entrées (L46–L49+) capitalisant les surprises de chaque semaine.

Le chantier est **partiellement réussi** si trois scénarios sur quatre passent. Acceptable : S2 est explicitement identifié comme le scénario à risque (instabilité LLM). Documenter l'écart honnêtement dans le rapport final.

Le chantier est **à reconsidérer** si la semaine 2 dépasse 10 jours réels — c'est le signe que `agent_infer` async + cancellation est plus dur que prévu et qu'il faut soit (a) demander de l'aide externe, soit (b) accepter une version dégradée bloquante (synchronisée par mutex au lieu d'async pur).

---

## 8. Méthode de travail

- **Granularité de commit.** Un commit par sous-livrable (chaque ADR, chaque brique, chaque scénario). Messages descriptifs.
- **TDD obligatoire sur les host functions.** Test unitaire de la host function avant son utilisation dans un scénario. Le scénario est un test d'intégration, pas un test unitaire de la host function.
- **Test de cancellation prioritaire.** Le test unitaire de cancellation de `agent_infer` (Q5.1 d'ADR-0019) doit être écrit en semaine 2 et rester vert tout au long du chantier. C'est l'invariant le plus critique.
- **Pas de nouveaux dépendances sans justification.** Si un crate est ajouté à `Cargo.toml`, écrire pourquoi dans le commit. `tokio-util` pour `CancellationToken` est justifié par ADR-0019 §Q5.1.
- **LESSONS au fil de l'eau.** Chaque surprise (technique, conceptuelle) qui prend plus d'une demi-journée à résoudre devient une entrée LESSONS. Ne pas attendre la fin.
- **Auto-critique honnête.** Si un scénario "marche" mais que le code embarque un hack (`sleep` magique, mock qui contredit l'intention, etc.), c'est documenté comme dette même si le test est vert.

---

## 9. Questions ouvertes pour l'agent CLI

Si l'une de ces questions devient bloquante, ne pas improviser — formuler la question avec contexte et options, demander arbitrage à l'humain avant de coder.

- **Q-OPEN-1.** Si Ollama renvoie une réponse JSON malformée (qwen2.5:3b a tendance à parfois entourer un JSON de prose), qui parse ? Le `agent_infer` host-side, ou l'agent WASM lui-même ? Recommandation par défaut : l'agent. Mais à confirmer.
- **Q-OPEN-3.** Faut-il une primitive `agent_send(target_agent_id, payload)` pour permettre à un agent d'initier une communication non-supervision avec un autre agent ? Probablement pas dans ce chantier — on s'en tient à la communication via Scheduler et primitives existantes — mais à noter.
- **Q-OPEN-4.** Le scénario S3 doit-il faire de la mesure réelle (chrono) ou se contenter d'asserts d'état ? Recommandation : asserts d'état pour la robustesse, mesure chrono optionnelle pour la richesse du LESSONS.

**Q-OPEN résolues en v2 :**
- ~~Q-OPEN-2~~ (corrélation worker/supervisor) → résolue par D-F (supervisor algorithmique).
- ~~Q-OPEN-5~~ (déterminisme trace) → résolue : reproductibilité sémantique uniquement, snapshot des références (§5.3).
- ~~Q-OPEN-6~~ (hash vs inline) → résolue : inline borné 8 KB avec troncature (ADR-0019 §3.1.1).
- ~~Q-OPEN-7~~ (DoS pool) → résolue : double timeout, borne hôte non négociable (ADR-0019 §3.1.2).
- ~~Q-OPEN-8~~ (valeur ajoutée B6) → résolue : B6 reformulé en composition A1+A2 sur décision LLM (§4 semaine 4).

---

## 10. Ressources

- **Wasmtime Linker::func_wrap_async :** https://docs.rs/wasmtime/latest/wasmtime/struct.Linker.html#method.func_wrap_async
- **tokio_util::sync::CancellationToken :** https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html
- **Ollama API chat :** https://github.com/ollama/ollama/blob/main/docs/api.md#generate-a-chat-completion
- **qwen2.5:3b model card :** https://ollama.com/library/qwen2.5
- **WASI Preview 1 ABI :** https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md

---

## Annexe A — Structure cible du repo après le chantier

```
poc/
├── store/                  (existant)
├── causal-log/             (existant — ajout EmitType 0x0C/0x0D/0x0E/0x0F)
├── capabilities/           (existant)
├── runtime/                (existant — ajout agent_infer, WaitingInference, InferencePool avec CancellationToken)
├── reconstruct/            (existant — ajout rendu Inference* events)
├── agent-sdk/              (NOUVEAU — crate Rust→WASM)
│   ├── src/lib.rs
│   └── examples/
│       ├── echo.rs                  → echo.wasm (S0, validation pipeline)
│       ├── worker_prime.rs          → worker_prime.wasm (S1, LLM)
│       ├── supervisor_arith.rs      → supervisor_arith.wasm (S1, algorithmique)
│       ├── worker_double_check.rs   → worker_double_check.wasm (S2, A1+A2)
│       ├── density_worker.rs        → density_worker.wasm (S3)
│       └── rollback_target.rs       → rollback_target.wasm (S4)
└── scenarios/              (NOUVEAU)
    ├── S1-supervision-algorithmique/
    │   ├── README.md
    │   ├── run.sh
    │   ├── expected_log.txt
    │   └── reference_responses.jsonl
    ├── S2-self-rollback-incoherence/
    ├── S3-inference-cap/
    ├── S4-scheduler-rollback/
    └── run-all.sh

decisions/
├── 0019-primitive-agent-infer.md   (NOUVEAU — v2 enrichi)
├── 0020-toolchain-agent-sdk.md     (NOUVEAU)
└── 0021-convention-scenarios.md    (NOUVEAU)
```

---

## Annexe B — Glossaire de chantier

- **Agent WASM** : module `.wasm` compilé depuis Rust, exécuté dans un sandbox Wasmtime, utilisant les host functions A* + `agent_infer`.
- **Worker** : agent qui exécute une tâche métier (calculer, écrire dans le store). Peut être stochastique (LLM) ou déterministe.
- **Supervisor** : agent qui répond aux `ValidationRequest`. Pas de différence structurelle avec un worker — c'est juste un agent avec des capabilities différentes et un rôle orienté validation. Peut être algorithmique (S1) ou LLM (hors scope de ce chantier).
- **Scénario** : test d'intégration end-to-end auto-évalué. Setup → run → asserts → rapport. Pas de présence humaine requise.
- **PoC bout-en-bout** : ensemble des quatre scénarios + harness + reconstruct. C'est le livrable de ce chantier.
- **Reconstruct** : exécution post-hoc de `os-poc-reconstruct` pour traduire le log causal en chronologie lisible humain. Seul point de contact humain avec le système.
- **Borne dure (pool d'inférence)** : limite stricte sur le nombre d'inférences concurrentes, imposée par un sémaphore Tokio. Distincte des propriétés fortes de C1 (équité, priorité sémantique) qui restent Phase 6.
- **Reproductibilité sémantique** : un scénario est reproductible si son verdict (pass/fail) ne dépend pas de la formulation exacte des réponses LLM, mais uniquement de l'état final observable (contenu store, séquence d'EmitType).
- **Cancellation** : mécanisme par lequel une Future Tokio en cours d'exécution est interrompue proprement, son slot libéré, et un événement `InferenceCancelled (0x0E)` tracé dans le log causal.

---

*Fin du briefing v2. Bonne route.*