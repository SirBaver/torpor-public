# ADR-0052 — Scope Phase 10 : exercer le scheduler d'inférence sous backend réel

**Date :** 2026-05-30
**Statut :** Acceptée

---

## Contexte

La campagne adversariale (ADR-0050/0051) est close. ADR-0050 §D6 avait coupé l'axe plafonds C1/C2/C3 en posant deux conditions de réouverture **conjonctives** :

> « Renvoyés à une campagne ultérieure conditionnée à : inférence réelle **et** cap C2 recalibré sur hardware représentatif. »

Un fait nouveau rend la première condition atteignable immédiatement : `OllamaBackend` est intégralement implémenté dans `poc/runtime/src/inference/mod.rs` (backend HTTP `reqwest` vers Ollama, modèle `qwen2.5:3b`, timeout + CancellationToken). De plus, l'inspection du code révèle que le « gestionnaire de slots d'inférence » décrit en spec/07 §2 comme « Design requis » est en réalité **implémenté depuis Phase 6** (`InferenceQueue`, ADR-0022/0023) — le label était stale (corrigé dans le commit de cohérence précédant cet ADR).

La seconde condition d'ADR-0050 §D6 (recalibrage C2 sur hardware représentatif) reste non disponible (mur d'infra, groupe de dette #8/D-P3a/β-seL4). Phase 10 **ne prétend donc pas clore la condition conjonctive** d'ADR-0050 §D6 — elle lève uniquement la moitié inférence réelle, et l'inscrit explicitement comme telle.

---

## Décision

### D1 — Périmètre

Phase 10 n'est **pas** une « requalification de C1 » au sens plein d'ADR-0050 §D6 (la condition C2/hardware reste ouverte). Son objet est précis :

**Exercer le scheduler d'inférence existant sous un backend d'inférence réel, et falsifier E1, E3 et P1b sous ce régime.**

Composants concernés (tous existants, rien à concevoir) :
- `InferenceQueue` (ADR-0022/0023) — priorité Supervisor > Foreground > Batch, famine bornée
- Coordination C1↔C2 (ADR-0030) — pipeline `slot_freed_notify` sous charge LLM réelle
- Cycle evict/wake (ADR-0031) — agents `Suspended` reprennent à la libération d'un slot
- Scénarios S3 (`inference-cap`) et S5 (`fairness-priority`) — existent, tournaient sous `SleepyBackend`

Livrables : (1) brancher `OllamaBackend` sur les scénarios existants ; (2) mesurer E1/E3/P1b + coordination C1↔C2 sous backend réel.

**Aucun livrable de design.** Phase 10 est entièrement un chantier de mesure et de branchement.

### D2 — Définition opérationnelle de « inférence réelle »

**Backend retenu : `OllamaBackend` / `qwen2.5:3b` sur le poste de développement (option (a)).**

Rejetés :
- *(b) Backend calibré* (sleep avec distribution réaliste) — ne lève pas F1 ; c'est un stub plus sophistiqué, pas de l'inférence réelle. ADR-0050 rejette « saturer un stub » (§D6 F1).
- *(c) Hardware cible GPU 24 GB* — mur d'infra non disponible.

**Garde-fou de non-transférabilité (calque D7/ADR-0050) :**
Tout débit, toute latence et tout `t_inférence` mesurés en Phase 10 sont **non transférables au hardware cible** (GPU 24 GB, L9). Ils caractérisent le comportement du scheduler C1 sous le backend `OllamaBackend/qwen2.5:3b` sur le poste de développement — pas le débit absolu de la spec. Les valeurs k=4–8 et t≈2,5 s de spec/07 §2 restent des **hypothèses de dimensionnement non validées** après Phase 10. Tout verdict nomme le backend et le hardware explicitement ; jamais « C1 » nu.

### D3 — Critère de falsification

C1 est un *plafond* (borne descriptive), pas une propriété P1–P6. On ne falsifie pas un plafond en isolation. Le critère porte sur les **propriétés du scheduler qui réalisent le plafond** :

**E1 + E3 (ADR-0023 D1) sous inférence réelle.**
Oracle : `queue_stats()` / `QueueTrace { admission_ts, slot_acquired_ts, priority_class_at_admission, promoted_from }` dans `poc/runtime/src/inference/queue.rs`. Le système ÉCHOUE si A-priorité, A-E1 ou A-E3 sont violés sous charge OllamaBackend.

**P1b sous inférence réelle.**
La densité active (agents inférant simultanément) est bornée par les slots. P1b est réfutée si E1 ou E3 cassent sous charge, ou si la densité active observée dépasse la borne « slots saturés ».

**Coordination C1↔C2 sous charge LLM réelle (spec/07 §3.4).**
Oracle : vérifier que le preload C2 ne devance pas la disponibilité des slots C1 de manière systématique (gaspillage de cache). Observation, non bloquante pour le verdict global.

**Overhead du scheduler (observation, non pass/fail) :**
Mesurer `débit_observé` vs `k / median(t_infer_mesuré)` pour borner l'overhead de file. Ne pas comparer à la constante 2,5 s de spec/07 §2 (hypothèse non représentative sur ce hardware, piège L32).

**Correction par rapport au brief Phase 10 :**
Les compteurs `pop_with_sup_present` / `sup_chosen_when_present` vivent dans `io_queue.rs` (C2 — `IoAdmissionQueue`), **pas** dans l'`InferenceQueue` (C1). L'oracle d'inversion de priorité C1 est l'assertion A-priorité de S5 sur la trace `QueueTrace` — à réutiliser et étendre, pas à recréer depuis C2.

### D4 — Statut gestionnaire de slots / spec/07 §2

Le gestionnaire de slots d'inférence décrit en spec/07 §2 est **entièrement implémenté** :
- Priorité + famine : ADR-0022/0023 (`InferenceQueue`)
- Cycle evict/wake : ADR-0031 (`SchedulerCoordinator`)
- Coordination C2→C1 : ADR-0030 (pipeline `slot_freed_notify`)

Phase 10 n'a **aucun design à produire**. Le seul résiduel constructible est la mesure de la coordination C1↔C2 sous charge LLM réelle (risque spec/07 §3.4, nommé en D3).

Le label « Design requis. Non encore schedulé » de spec/07 §2 a été corrigé dans un commit autonome précédant cet ADR (« Implémenté Phase 6, ADR-0022/0023 ; evict/wake Phase 7, ADR-0031 »). ADR-0052 référence cette correction sans la contenir.

### D5 — Non-objectifs de Phase 10

| Item | Nature | Raison du gel |
|------|--------|---------------|
| Recalibrage C2 / cap matériel | Moitié manquante d'ADR-0050 §D6 | Hardware NVMe serveur PCIe Gen4 non disponible |
| Commit cross-store atomique (#7b) | Déclencheur GC non atteint | ADR-0051 §D4 — palliatif #7a en place |
| Durabilité power-loss / β seL4 / D-P3a (#8) | Mur d'infra media réel | ADR-0046, ADR-0051 §D4 |
| B-fort multi-tenant | Déclencheur 2e TenantId non atteint | ADR-0036 §sortie |
| C.12+ seL4 (setjmp, watchdog, fuel, signature) | Déclencheurs non atteints | ADR-0048 §D6, ADR-0049 §D3 |
| GC orphelins / re-séparation CAS-index | Croissance non bornée non observée | ADR-0046 §42, ADR-0049 §D3a |
| Campagne P2/P3/P5 adversariale | Non annulée, non urgente | ADR-0050 §D6, ADR-0051 §D5 |

La condition ADR-0050 §D6 (conjonctive) reste **partiellement ouverte** : la moitié inférence réelle est levée par Phase 10 ; la moitié C2/hardware attend le substrat représentatif.

---

## § Clôture sous-axe B — oracle P5 #3 non déclenché (2026-05-30, Phase 11 T1)

Le sous-axe B (D2 §Conséquences l.114) était conditionnel : *si* l'inférence réelle crée un agent de référence consommant une primitive non-déterministe (sortie LLM), *alors* la dette d'oracle P5 #3 (ADR-0051 §D5) devient exerceable et un scénario SEF-6-bis doit être ouvert.

**Verdict : déclencheur NON atteint. SEF-6-bis NON requis.**

### Constat de fait (audit du chemin de hash, `actor.rs:1267-1317`)

L'état committé par `commit_barrier` est hashé via une préimage **figée** :

```
state_bytes = [ agent_id (16 B) | seq (8 B) | zéros (40 B) ]
data_hash   = put_block(state_bytes)
snap_id     = put_snapshot(SnapshotHeader { data_hash, parent: last_snapshot, seq, ts_us })
```

- `data_hash` ne dépend que de `(agent_id, seq)`.
- `snap_id` ne dépend que de `(data_hash, parent_chain, seq, ts_us)`.
- `ts_us` provient de `clock.now_us()` ; sous SEF-6, `LogicalClock` (`clock.rs:93-99`) en fait un compteur déterministe par séquence d'appels.

La sortie LLM (`InferResponse.text`, `inference/mod.rs:51-56`) n'entre dans **aucun** de ces champs. `OllamaBackend` retourne `text` à l'agent WASM via `agent_infer`, mais le hash d'état suivant est identique run-à-run quelle que soit la réponse du modèle.

P5 porte sur le **déterminisme de la transition d'état** (le hash), pas sur la sémantique du contenu émis. L'agent INFER_WAT_REAL appelle `agent_infer → commit_barrier → emit` ; il *émet* la sortie LLM (canal observable hors-état) mais ne la fait pas entrer dans le préimage hashé. Le mécanisme S6 (`LogicalClock`) demeure l'unique source de variabilité de transition, et il est substituable et déterministe. **SEF-6 reste valide ; le branchement d'OllamaBackend ne le falsifie pas.**

### Correction d'une imprécision de l'analyse Phase 10 (TODO sous-axe B)

L'analyse Phase 10 affirmait que fermer la dette #3 « demande un agent *stockant* la réponse LLM dans son état (`kv_store`) ». **C'est inexact dans l'implémentation actuelle.** `kv_store` (`actor.rs:657`, peuplé par `agent_store_put` l.1713) n'est **jamais** sérialisé dans `state_bytes`. Stocker la sortie LLM via `agent_store_put` ne la ferait pas davantage entrer dans `data_hash`.

Le déclencheur réel de #3 n'est donc pas « un agent qui stocke dans `kv_store` » mais l'une des deux conditions, cohérentes avec ADR-0051 §D5 :

1. **Modification de `commit_barrier`** pour élargir la préimage `state_bytes` au-delà de `(agent_id, seq)` — c.-à-d. faire entrer dans `data_hash` une valeur dérivée d'une primitive non-déterministe (sortie LLM, `kv_store`, entropie, etc.). C'est un changement de code structurant, pas un simple agent de référence.
2. **Campagne P2/P3/P5 dédiée** (ADR-0050 §D6), qui construirait l'oracle adéquat.

Tant qu'aucune des deux n'est instanciée, #3 reste une **dette d'oracle dormante de basse priorité** (ADR-0051 §D5, statut inchangé). Phase 10 ne l'a pas réveillée.

### Cohérence ADR

- **ADR-0051 §D5** : respecté. §P5 spec reste correcte (garantie conditionnelle à S6) ; le défaut reste dans l'oracle SEF-6, non dans la spec ; aucune édition spec/02. Le déclencheur (campagne P5 dédiée *ou* agent faisant entrer une primitive non-déterministe dans l'état hashé) demeure non atteint. Cet amendement **ne rouvre pas** ce qui a été différé.
- **ADR-0001** (P5 en avant-dernier rang de priorité) : confirmé — la dette reste de basse priorité.

**Action TODO** : passer le sous-axe B de `[~]` à `[x]` avec renvoi à cette section. Ne pas ouvrir SEF-6-bis.

---

## Scénarios de Phase 10

Deux scénarios existants à ré-exécuter avec `OllamaBackend` à la place de `SleepyBackend` :

| Scénario | Ce qu'il teste | Oracle |
|----------|----------------|--------|
| S3 `inference-cap` | Borne dure k slots, WaitingInference, absence de famine | `QueueTrace.slot_acquired_ts`, log `0x0C–0x0F` |
| S5 `fairness-priority` | A-priorité (Supervisor < Foreground), A-E1 (FIFO intra-classe), A-E3 (famine ≤ 30 s) | `queue_stats()`, `QueueTrace`, assertions ADR-0023 |

Scénario de mesure de coordination C1↔C2 (optionnel, observation) : à instruire si S3/S5 révèlent du gaspillage de preload C2 sous charge LLM variable.

---

## Conséquences

- **TODO.md** : nouvelle section Phase 10 avec deux axes (A: branchement + S3/S5 sous OllamaBackend ; B: dette oracle P5 #3 si inférence réelle crée un agent non-déterministe).
- **spec/07 §2** : label corrigé (commit autonome, voir git log). Hypothèses k/t restent non validées — à annoter dans les verdicts S3/S5.
- **ADR-0050** : condition §D6 partiellement levée (moitié inférence réelle). Non soldé — la moitié C2/hardware reste. ADR-0052 ne clôt pas ADR-0050 §D6.
- **ADR-0022/0023/0030/0031** : non amendés — Phase 10 les *exerce*, ne les modifie pas.
- **Sous-axe B (oracle P5 #3)** : conditionnel. Si OllamaBackend crée un agent de référence consommant une primitive non-déterministe (sortie LLM), la dette d'oracle #3 devient exerceable. **Évalué en Phase 11 T1 : déclencheur NON atteint — la sortie LLM n'entre pas dans le préimage du hash d'état, le hash de transition reste déterministe. SEF-6-bis non ouvert.** Voir § Clôture sous-axe B ci-dessus.

---

## Références

- `decisions/0050-campagne-mise-a-lepreuve.md` §D6 (condition conjonctive, F1, méthode critère-avant-code)
- `decisions/0051-cloture-campagne-tri-findings.md` §D5 (#3 oracle P5), §D4 (non-objectifs)
- `decisions/0022-inference-queue.md`, `decisions/0023-inference-famine.md` (E1, E3, A-priorité)
- `decisions/0030-scheduler-unifie.md` (pipeline C2→C1, `slot_freed_notify`)
- `decisions/0031-scheduler-coordinator.md` (evict/wake, `SchedulerCoordinator`)
- `decisions/0001-priorite-proprietes.md` (P4 ≻ P2 ≻ P3 ≻ P6 ≻ P5 ≻ P1)
- `spec/07-plafonds-architecturaux.md` §2 (C1, hypothèses k/t), §3.4 (risque preload C2 sans slot C1)
- `poc/runtime/src/inference/mod.rs` (`OllamaBackend` l.163+, `InferencePool`)
- `poc/runtime/src/inference/queue.rs` (`InferenceQueue`, `QueueTrace`, `queue_stats()`)
- `poc/runtime/src/io_queue.rs` (`IoAdmissionQueue`, compteurs C2 — **distincts de C1**)
- `poc/scenarios/S3-inference-cap/` (borne dure k slots)
- `poc/scenarios/S5-fairness-priority/` (E1/E3/A-priorité, oracle A-priorité)
- `lab/LESSONS.md` L32 (page-cache/stub masque le signal), L68 (critère de falsification avant le code)
