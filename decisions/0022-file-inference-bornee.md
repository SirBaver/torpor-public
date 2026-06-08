# ADR-0022 — File d'inférence bornée et discipline d'ordonnancement

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

ADR-0019 §Q6 (D-Q-V2.6) a accepté en Phase 2 un `tokio::sync::Semaphore`
non borné pour gérer le pool d'inférence (cap=4 slots). Conséquences
opérationnelles inscrites dans la dette :

- File d'attente illimitée — empreinte mémoire non bornée en charge
  soutenue.
- Code retour `NoSlot (3)` réservé dans l'ABI WASM mais jamais émis.
- Pas de discipline d'ordonnancement explicite : le `Semaphore` Tokio
  servait les attentes dans l'ordre approximatif des `acquire().await`,
  sans garantie FIFO formelle et sans notion de priorité.

Le brief Phase 6 (`docs/archive/phase6.md §3.1`) demande de trancher :

1. **Q-Ph6-1** Discipline de service (FIFO / priorité multi-niveau /
   pondéré fair-share).
2. **Q-Ph6-2** Politique de rejet quand la file est pleine
   (drop-newest / drop-oldest / drop par priorité / backpressure).
3. **Q-Ph6-3** Taille de file par défaut (`queue_capacity`).
4. **Q-Ph6-4** Visibilité de l'attente dans le log causal (nouveau
   EmitType vs enrichissement de `0x0C InferenceRequest`).

Contraintes structurantes héritées :

- **D-Ph6-A.** ABI `agent_infer` figée. Le code retour `NoSlot (3)` est
  activé (passage de "réservé" à "actif"), aucun autre changement. Pas
  de nouveau code retour, pas de nouveau paramètre.
- **D-Ph6-G.** Pas de nouvelle host function exposée à l'agent.
- **`spec/07 §C1.3`** demande une priorité sémantique
  `supervisor > foreground user > batch`.
- **ADR-0019 §Q-V2.3** invariant `tokio::select! { biased; ... }` —
  exactement une branche gagne. Préservé : la file ne change pas la
  sémantique d'exécution une fois le slot acquis.

Forces en présence :

- FIFO strict est simple et auditable, mais ne matérialise pas §C1.3 —
  un supervisor attend derrière un batch.
- Priorité multi-niveau matérialise §C1.3 mais introduit le risque de
  famine pour les classes basses.
- Pondéré (CFS-like, stride scheduling [Waldspurger 1995]) demande de
  calibrer poids et fenêtre — paramètres pour lesquels Phase 6 n'a pas
  de données de production.

---

## Décision

Quatre sous-décisions D1–D4, à appliquer conjointement.

### D1. Discipline : **priorité multi-niveau stricte à trois classes avec garde-fou de famine bornée**

Le sémaphore Tokio plat est remplacé par une structure `InferenceQueue`
maintenant **trois files FIFO** indexées par classe :

```text
PriorityClass ∈ { Supervisor, Foreground, Batch }
```

- `Supervisor` : agent répondant à un `ValidationRequest` (A3) ou
  agent déclaré `AgentProfile::Algo` faisant office de superviseur
  (cf. ADR-0025). Servi en priorité absolue.
- `Foreground` : agent interactif (cycle court, latence visible côté
  utilisateur).
- `Batch` : traitement long, latence non critique.

**Règle de service :**

1. À chaque libération d'un slot, l'`InferenceQueue` tire la tête de
   `Supervisor` si non vide, sinon `Foreground`, sinon `Batch`.
2. À l'intérieur d'une classe, l'ordre est FIFO strict sur le
   timestamp d'admission (`admission_ts`).
3. **Garde-fou de famine bornée.** Toute requête en attente depuis
   plus de `max_starvation_ms` est **promue d'un cran** vers la
   classe supérieure (`Batch → Foreground`, `Foreground → Supervisor`).
   La promotion est **au plus une fois par requête** — une requête
   promue ne peut être promue à nouveau. Valeur par défaut :
   `max_starvation_ms = 10_000` (10 s).

**Justification.** Cette discipline est la formulation la plus proche
de `spec/07 §C1.3` sans introduire de paramètres CFS/stride qu'on n'a
pas les données pour calibrer. Le garde-fou répond directement à
`§C1.4` (absence de famine bornée). La limitation à une promotion par
requête évite la boucle de promotion identifiée au brief §5.4.

**Source de la classe d'une requête.** La classe est déterminée à
l'admission, dans cet ordre :

1. Si l'agent est en train de répondre à un `ValidationRequest` A3
   (état `AwaitingValidation` sur un agent enfant) → `Supervisor`.
2. Sinon, selon le `risk` level passé à `agent_request_validation`
   par le parent qui a engendré l'agent (`risk = Critical` →
   `Supervisor`, `risk = Normal` → `Foreground`, `risk = Low` →
   `Batch`).
3. Sinon, défaut `Foreground`.

La classe n'est **pas** un paramètre de l'ABI `agent_infer` (D-Ph6-A) :
elle est dérivée par l'hôte à partir d'état déjà connu. Cohérent avec
ADR-0014 §politique de supervision (le `risk` level structure déjà la
suite logique).

### D2. Politique de rejet : **drop-newest avec dégradation par priorité**

Quand la file est pleine (somme des trois sous-files = `queue_capacity`)
et qu'une nouvelle requête arrive :

1. **Si la nouvelle requête est de classe `Supervisor`** et qu'au
   moins une requête de classe `Batch` est en attente : la plus
   récente requête `Batch` est évincée — son `agent_infer` retourne
   `NoSlot (3)`, et un `InferenceFailed (0x0F)` avec `error_code =
   0x20 (NoSlot)` est émis. La nouvelle requête est admise.
2. **Sinon** : la nouvelle requête est refusée immédiatement
   (`try_acquire` échoue) — son `agent_infer` retourne `NoSlot (3)`,
   un `InferenceFailed (0x0F)` avec `error_code = 0x20` est émis.

Aucune éviction d'une requête `Foreground` ou `Supervisor` déjà admise.
L'éviction est limitée à la classe `Batch` strictement. Une requête
`Foreground` arrivant sur une file pleine de `Foreground` + `Batch`
est refusée (pas d'éviction inter-classe au-delà de Batch).

**Justification.**

- Drop-newest pur (option α du brief) gèle la file aux anciens et
  bloque les supervisors arrivés en retard — viole §C1.3.
- Drop-oldest inconditionnel (option β) trahit l'agent évincé qui
  attendait — son `agent_infer` retourne `NoSlot` après un délai
  d'attente non nul, comportement contre-intuitif.
- Dégradation par priorité (option γ du brief) appliquée uniquement
  à la classe `Batch` : cohérent avec la priorité multi-niveau (D1),
  pas de comportement étrange pour les classes `Foreground` et
  `Supervisor` qui restent en politique drop-newest.

**Pas de retry automatique côté hôte** (cohérent ADR-0014 §c, ADR-0019
§Q4). L'agent qui reçoit `NoSlot (3)` décide : retry, terminer, ou
self_rollback. La politique post-`NoSlot` reste côté agent.

### D3. Capacité par défaut : `queue_capacity = 4 × max_concurrent_inferences`

Valeur par défaut : **16** (avec `max_concurrent_inferences = 4`).
Configurable au démarrage du scheduler.

**Justification.** Le brief §3.1 Q-Ph6-3 suggérait 32 (`8 ×
max_concurrent`) en se basant sur un burst absorbé à débit nul de
~0,8 s pour `t_infer = 100 ms`. Revu à `4 ×` après analyse :

- À `t_infer = 2,5 s` (qwen2.5:3b), une file de 16 absorbe un burst de
  10 s à débit nul — déjà au-delà du `max_wait_ms = 30 s` qu'ADR-0023
  va fixer. Au-delà, on ajoute de la mémoire qui sera de toute façon
  rejetée par la borne d'attente.
- L'empreinte mémoire `4 × 4 × ~1 KB` = ~16 KB par scheduler reste
  marginale, mais croît linéairement avec `max_concurrent`. À cap=100
  (Phase 7+), `4 × 100 × 1 KB` = 400 KB — tolérable.
- Pour la classe `Batch` exclusivement, un sous-quota peut être
  introduit en Phase 7 si le besoin émerge ; en Phase 6, la file
  totale est partagée.

Le ratio `4 ×` est une borne empirique, à confirmer par mesure S5.
S'il s'avère trop bas (rejets fréquents sous charge légitime), il sera
revu — pas par amendement d'ADR, mais par changement de paramètre par
défaut documenté en LESSONS.

### D4. Observabilité dans le log : **enrichissement de `InferenceRequest (0x0C)`**, pas de nouveau EmitType

Le payload de `0x0C InferenceRequest` est étendu, **en queue de
payload**, par deux champs :

```text
[ existing payload v1 ]                       <- ADR-0019 §Q3 inchangé
[ priority_class u8 ]                         <- 0x01 Supervisor, 0x02 Foreground, 0x03 Batch
[ queue_depth_at_admission u16 LE ]           <- profondeur totale de la file au moment de l'admission
[ promoted_from u8 ]                          <- 0x00 si admise directement,
                                                  0x01 si promue depuis Batch,
                                                  0x02 si promue depuis Foreground
```

**Compatibilité.** L'enrichissement est en queue de payload — les
decoders existants (Phase 2) lisent les champs ADR-0019 §Q3 dans
l'ordre fixé et ignorent les bytes supplémentaires. À vérifier en
Semaine 1 (Q-OPEN-Ph6-4 du brief) : si le decoder MessagePack actuel
échoue sur des bytes en surplus, la décision se replie sur l'option 3
(nouveau EmitType `0x10 InferenceAdmitted` distinct) — la décision D4
**ne change pas** dans son intention, seulement le mécanisme de
codage. Le test de compatibilité est inscrit comme critère bloquant
de Semaine 1.

**Pas de nouveau EmitType `InferenceQueued` ou `InferenceAdmitted` en
Phase 6.** Justification : la *séquence canonique* d'observation reste
celle d'ADR-0019 §Q-V2.1 (`0x0C → 0x0D/0x0E/0x0F`). Une entrée
`InferenceQueued` antérieure à `0x0C` doublerait l'information sans
ajouter de pouvoir d'analyse — la profondeur d'admission est déjà
suffisante pour reconstruire l'historique d'attente via les
`admission_ts` corrélés.

**Métrique runtime parallèle.** L'`InferenceQueue` expose un snapshot
synchrone via `Scheduler::queue_stats() -> QueueStats { per_class:
[count; 3], oldest_wait_ms_per_class: [u64; 3], total_admitted_since:
u64, total_rejected_since: u64 }`. Sert le test S5 (assertions
déterministes — cf. ADR-0023 §D3) et n'est pas inscrit dans le log
causal (introspection ad-hoc, pas trace pérenne).

### Relation avec `NoSlot (3)`

`NoSlot (3)` est émis dans **exactement deux cas** :

1. Refus immédiat par D2 (file pleine, requête arrivante refusée).
2. Éviction par D2 (file pleine, requête `Batch` évincée par un
   `Supervisor` arrivant).

Dans les deux cas, `InferenceFailed (0x0F)` est émis avec
`error_code = 0x20`. La séquence canonique d'observation pour un
refus immédiat est :

```text
[ pas de 0x0C ] → InferenceFailed (0x0F, error_code=0x20) → return NoSlot(3) côté WASM
```

Pour une éviction :

```text
... → InferenceRequest (0x0C, agent X, priority_class=Batch) →
      [agent X reste en attente] →
      InferenceFailed (0x0F, agent X, error_code=0x20) →
      InferenceRequest (0x0C, agent Y, priority_class=Supervisor) → ...
```

Le `0x0C` de la requête évincée a déjà été émis à son admission. Son
`0x0F` postérieur signale l'éviction. Cohérent avec ADR-0019 §Q-V2.7
(`seq` ne progresse pas sur Inference* events).

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A1. FIFO strict global (Q-Ph6-1 A)** | Plus simple, traçabilité ordinale triviale | Ne matérialise pas `spec/07 §C1.3` ; un supervisor attend derrière des batch | Rejetée. Le brief Phase 6 demande explicitement la priorité sémantique. |
| **A2. Pondéré fair-share / lottery (Q-Ph6-1 C)** | Évite la famine sans garde-fou ad-hoc ; expose des poids | Demande de calibrer poids et fenêtre ; pas de données pour le faire en Phase 6 ; surface d'API plus large | Rejetée. Reporté Phase 7+ quand données de production disponibles. [Waldspurger 1995] reste référence pour cette évolution. |
| **A3. Drop-newest pur (Q-Ph6-2 α)** | Stabilité de la file, simple | Un supervisor arrivant après une file pleine de batch est refusé — viole §C1.3 sous charge soutenue | Rejetée. Incohérent avec D1. |
| **A4. Drop-oldest (Q-Ph6-2 β)** | Borne dure sur le temps de séjour de toute requête | L'agent évincé a déjà attendu, son `NoSlot` arrive tardivement — comportement étrange | Rejetée. Préfère la dégradation ciblée sur Batch (D2). |
| **A5. Backpressure (Q-Ph6-2 δ)** | Pas de rejet, simple | Équivaut au sémaphore non borné actuel — annule le livrable | Rejetée. Contradiction avec la dette D-Q-V2.6. |
| **A6. Nouveau EmitType `InferenceQueued (0x10)`** (Q-Ph6-4 option 3) | Trace explicite et distincte de l'admission | Double l'information avec `0x0C` ; ajoute une migration de schéma | Rejetée par défaut. Fallback si l'enrichissement de `0x0C` (D4) s'avère casser le decoder MessagePack — décision conditionnelle. |
| **A7. Promotion multi-cran (Batch → Foreground → Supervisor en deux promotions)** | Anti-famine plus forte | Boucle de promotion : un batch promu en supervisor est servi avant un foreground arrivé après — inversion de priorité observable | Rejetée. Limitation à une promotion par requête (D1) explicite. |
| **A8. Sous-quotas par classe** (e.g. file de 8 réservée à Supervisor, 8 à Foreground, 4 à Batch) | Garanti des slots pour les classes hautes même si Batch sature | Plus complexe ; mauvais usage si Supervisor sous-utilisé pendant que Batch refuse | Rejetée. Une file totale partagée + politique D2 suffit pour les cas testés en Phase 6. Reporté Phase 7+. |
| **D1–D4 retenus** | — | — | Retenus |

---

## Conséquences

**Positives :**

- `spec/07 §C1.3` (priorité sémantique) matérialisée et testable via S5.
- `spec/07 §C1.4` (absence de famine bornée) adressée via la promotion
  bornée — testable par `t_promotion_is_bounded_one_step`.
- Empreinte mémoire de la file maîtrisée : `queue_capacity × ~1 KB` ≈ 16 KB
  par scheduler par défaut.
- Code retour `NoSlot (3)` activé sans changement d'ABI (D-Ph6-A
  préservée).
- Observabilité préservée : `priority_class`, `queue_depth_at_admission`,
  `promoted_from` rendent l'attente lisible dans le log.
- `Scheduler::queue_stats()` permet des assertions déterministes dans
  S5 sans dépendre du sampling Tokio (cf. ADR-0023 §D3).

**Négatives / coûts acceptés :**

- L'implémentation de l'`InferenceQueue` ne peut plus s'appuyer sur le
  `tokio::sync::Semaphore` natif (qui ne supporte pas la priorité). Il
  faut une structure manuelle : `Mutex<{ supervisor: VecDeque, foreground:
  VecDeque, batch: VecDeque, in_flight: u32 }>` + `Notify`. Surface
  d'attaque pour data races — tests d'intégration concurrence requis
  (cf. risque Semaine 1 du brief).
- L'éviction d'une requête `Batch` admise (D2) peut surprendre l'agent
  évincé. Documenté côté SDK : `agent_infer` peut retourner `NoSlot`
  même après acceptation préliminaire — l'agent voit ça comme un
  refus standard.
- La classe est dérivée par l'hôte (pas paramètre d'ABI). Un agent qui
  voudrait s'auto-déclasser ou se prioriser ne peut pas le faire via
  `agent_infer`. C'est intentionnel — la priorité est une décision
  scheduler, pas agent. Cohérent avec D-Ph6-G.
- L'enrichissement de `0x0C` (D4) ajoute 4 bytes par entrée
  (1+2+1). Négligeable face au header MessagePack + `prompt_hash`.
- Le ratio `queue_capacity / max_concurrent` est figé à 4× par défaut ;
  un mauvais choix se voit en production via le taux de rejet, pas par
  régression de test.

**Neutres / à surveiller :**

- L'enrichissement de payload `0x0C` doit être validé compatible avec
  le decoder MessagePack existant dès Semaine 1. Sinon, fallback A6
  (nouveau EmitType `0x10`) — décision conditionnelle, non bloquante
  pour le mergé de cet ADR.
- La dérivation automatique de la classe via le `risk` level (D1)
  suppose que les agents existants utilisent `agent_request_validation`
  pour leurs cycles. Les agents qui n'utilisent pas A3 retomberont sur
  le défaut `Foreground` — comportement par défaut acceptable mais à
  documenter dans le SDK.
- À l'observation S5, si le taux d'éviction de `Batch` par `Supervisor`
  est élevé (> 20 % des requêtes Batch sous charge), c'est un signal
  pour Phase 7 : soit sous-quotas par classe (A8), soit pondéré (A2).
- Si Tokio scheduler non déterministe rend la priorité observable
  flaky malgré l'instrumentation runtime, voir Q-OPEN-Ph6-1 du brief
  (passage à `tokio::runtime::Builder::new_current_thread()` pour les
  tests S5).

---

## Références

- ADR-0019 — Primitive `agent_infer` (ABI figée, codes retour, séquence
  canonique `0x0C → 0x0E → 0x0B`, §Q6 réserve de `NoSlot`)
- ADR-0014 — Politique supervision (timeout fixe, pas de retry — cohérent
  avec drop-newest)
- ADR-0010 — Contrat `emit` (EmitType, format MessagePack — autorise
  les types `0x0B–0xFF` réservés ; D4 ne demande pas de nouveau type)
- ADR-0017 — BlobDB sur CF `default` (`InferenceRequest` reste inline,
  payload enrichi < 100 bytes, pas d'impact seuil 4 KB)
- ADR-0018 — `os-poc-reconstruct` (à étendre pour rendre lisibles
  `priority_class` et `queue_depth_at_admission` — couvert par Ph6-B11)
- ADR-0023 — Équité formelle (définit E1, E3, métrique `max_wait_ms`
  qui dépend de D1–D2)
- ADR-0025 — Profils watchdog (l'`AgentProfile::Algo` peut être un signal
  complémentaire pour dériver la classe `Supervisor` ; cf. ADR-0025 D1)
- `spec/07 §C1.3` — Priorité sémantique (matérialisée)
- `spec/07 §C1.4` — Absence de famine bornée (adressée via promotion D1)
- [Waldspurger 1995] *Stride scheduling: deterministic proportional-share
  resource management*, MIT/LCS/TM-528 — référence pour Phase 7+ (option A2)
- [Waldspurger & Weihl 1994] *Lottery Scheduling: Flexible Proportional-Share
  Resource Management*, OSDI '94 — référence alternative pondérée
- Linux CFS — https://www.kernel.org/doc/html/latest/scheduler/sched-design-CFS.html
- `docs/archive/phase6.md §3.1` — Énoncé des questions Q-Ph6-1 à Q-Ph6-4
- `TODO.md` D-Q-V2.6 — Dette tranchée par cet ADR

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
