# ADR-0023 — Équité formelle et borne d'attente

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

`spec/07 §C1.3–C1.5` énonce trois propriétés que le scheduler
d'inférence doit satisfaire :

- §C1.3 — priorité sémantique `supervisor > foreground > batch`,
- §C1.4 — absence de famine bornée,
- §C1.5 — latence d'attente bornée.

ADR-0022 a tranché la **mécanique** (priorité multi-niveau + garde-fou
de promotion + drop-newest avec dégradation Batch). Reste à fournir
les **définitions opérationnelles** qui rendent ces propriétés
mesurables et testables. Sans définition formelle, "équité" n'est pas
une propriété, c'est un vœu (cf. critique architecturale 2026-05-14).

Le brief Phase 6 §3.2 propose trois définitions candidates :

- **E1 (équité ordinale)** — FIFO strict intra-classe.
- **E2 (fair-share proportionnel)** — quotas par classe sur fenêtre
  glissante W.
- **E3 (absence de famine bornée)** — borne dure `max_wait_ms` sur
  toute requête servie.

Et trois questions :

- **Q-Ph6-5** Quelle combinaison retient-on ?
- **Q-Ph6-6** Valeur de `max_starvation_ms` (utilisée par ADR-0022 D1)
  et `max_wait_ms` ?
- **Q-Ph6-7** Comment mesurer la conformité dans S5 (runtime
  instrumentation vs reconstruction depuis log causal vs les deux) ?

Contraintes héritées :

- **ADR-0022** définit déjà la mécanique : trois classes, promotion
  bornée à un cran, drop-newest avec éviction Batch. Toute définition
  d'équité doit être réalisable par cette mécanique.
- **`spec/02 §4.3`** définit la liveness conditionnelle. E3 en est un
  raffinement chiffré.
- **Phase 6 ne reproduit pas T5** (D-Ph6-D) ; les valeurs chiffrées
  doivent être des bornes lâches qu'aucune mesure de production
  Phase 6 ne risque de violer artificiellement.
- **D-Ph6-F** Les 53 tests existants restent verts. Notamment S3
  (`inference-cap`) vérifie que `tous les workers finissent par
  compléter` ; cette propriété doit être préservée par toute définition
  retenue.

Forces en présence :

- E2 demande de fixer des poids par classe et une fenêtre W. Ces
  paramètres ne peuvent pas être calibrés en Phase 6 (pas de données
  de production). Les choisir arbitrairement (e.g. `weights = (3, 2,
  1)` sur W=10s) prétendrait à une garantie qu'on ne sait pas valider.
- E1 + E3 est testable de façon déterministe via les instruments
  runtime (cf. ADR-0022 D4 `Scheduler::queue_stats()`).
- E3 sans E1 laisserait la file libre de servir dans un ordre
  arbitraire intra-classe — perte de prévisibilité.

---

## Décision

Trois sous-décisions D1–D3.

### D1. Définition retenue : **E1 (équité ordinale intra-classe) + E3 (absence de famine bornée)**

**E1 — Équité ordinale intra-classe (énoncé opérationnel).**

> Pour toute paire de requêtes A et B de **même classe initiale**, si
> `admission_ts(A) < admission_ts(B)` et **aucune des deux n'est promue
> par le garde-fou de famine** d'ADR-0022 D1, alors
> `slot_acquired_ts(A) < slot_acquired_ts(B)` ou A est évincée (ADR-0022
> D2).

Mesure : extraire de l'instrumentation `InferenceQueue` les couples
`(admission_ts, slot_acquired_ts, priority_class_at_admission,
promoted_from)`. Pour chaque classe initiale, filtrer
`promoted_from = 0x00` (admises directement), trier par
`admission_ts` croissant, vérifier `slot_acquired_ts` monotone.

**E3 — Absence de famine bornée (énoncé opérationnel).**

> Toute requête `agent_infer` qui n'est pas refusée par `NoSlot (3)`
> obtient un slot en au plus `max_wait_ms` millisecondes après son
> `admission_ts`. Formellement :
>
> `∀ r : InferenceRequest. r.outcome ∈ {Ok, Timeout, Error} ⇒
>  r.slot_acquired_ts − r.admission_ts ≤ max_wait_ms`

Mesure : sur la même instrumentation, calculer `wait_time_ms =
slot_acquired_ts − admission_ts` pour toute requête servie (`r.outcome
∈ {Ok, Timeout, Error}`). Assert : `p100(wait_time_ms) ≤ max_wait_ms`.

**E2 (fair-share proportionnel) explicitement non retenue en Phase 6.**
Reportée Phase 7+. La justification, déjà au brief : poids et fenêtre
non calibrables en Phase 6. La mécanique d'ADR-0022 (priorité stricte
+ promotion bornée) ne contredit pas E2 ; elle est plus simple et
suffisante pour les propriétés C1.3–C1.5.

**Implication de D1.** Sous E1 + E3 et la mécanique d'ADR-0022 :

- §C1.3 (priorité sémantique) est satisfaite par la priorité stricte
  d'ADR-0022 D1 — le supervisor passe devant le batch parce que sa
  classe est servie en priorité, pas parce que E1/E3 le forcent.
- §C1.4 (absence de famine bornée) est satisfaite par E3 + garde-fou
  de promotion d'ADR-0022.
- §C1.5 (latence d'attente bornée) est satisfaite par E3
  directement.

E1 est la propriété de prévisibilité interne à chaque classe ; E3 est
la propriété de progression observable de bout en bout.

### D2. Valeurs chiffrées

| Paramètre | Valeur par défaut | Sens |
|-----------|-------------------|------|
| `max_wait_ms` | 30 000 (30 s) | Borne maximale d'attente d'une requête servie (E3). |
| `max_starvation_ms` | 10 000 (10 s) | Seuil de promotion d'ADR-0022 D1. **Doit être < `max_wait_ms`** (sinon la promotion ne sert jamais avant le rejet par E3). |
| `host_max_inference_duration_ms` | 60 000 (60 s) | Borne ADR-0019 §Q4. **Indépendant** de E3 — c'est la borne *après* obtention du slot, pas *avant*. |

**Configurabilité.** `max_wait_ms` et `max_starvation_ms` sont
paramètres de configuration du scheduler, lus au démarrage. Pas de
modification à chaud. Les valeurs par défaut sont inscrites dans le
code de `InferenceQueue`.

**Justification des valeurs.**

- `max_wait_ms = 30_000` : à `t_infer = 2,5 s` (qwen2.5:3b) et cap=4,
  le pire cas FIFO d'attente pour 32 requêtes en file est `32 × 2,5 /
  4 = 20 s`. Avec `queue_capacity = 16` (ADR-0022 D3), le pire cas
  descend à `16 × 2,5 / 4 = 10 s`. Borne 30 s = 3× la valeur
  asymptotique attendue — borne lâche pour tolérer la variabilité du
  backend. Si un test de S5 dépasse `max_wait_ms`, c'est un signal
  réel (pas un faux positif), pas un défaut de calibration.
- `max_starvation_ms = 10_000` : choisi inférieur à `max_wait_ms`
  d'un facteur 3 pour laisser au garde-fou le temps d'agir avant que
  E3 ne déclenche un échec test. Une requête `Batch` non promue après
  10 s sera donc servie avant 30 s (par éviction d'autres Batch ou
  par le retour des slots).
- Aucune relation à `host_max_inference_duration_ms` : ce dernier
  borne *l'exécution* de l'inférence après obtention du slot, pas
  *l'attente* du slot.

**Que se passe-t-il quand `wait_time_ms > max_wait_ms` ?**

Cas borderline. La mécanique d'ADR-0022 D2 ne refuse pas a posteriori
une requête en attente (`drop-newest`, pas `drop-on-timeout`).
Conséquences :

- En Phase 6, **E3 est une propriété d'assertion test**, pas une
  garantie runtime active. Un dépassement n'est observé qu'au test S5
  qui assert `p100 ≤ max_wait_ms`. La requête restera en file
  jusqu'à éviction ou jusqu'à obtenir son slot.
- Phase 7+ pourra introduire une éviction par `max_wait_ms` (avec
  retour `NoSlot` à l'agent qui attendait). Cette extension est
  cohérente avec la mécanique actuelle — pas de changement d'ABI.

Cette tension (assertion test vs garantie runtime) est inscrite
explicitement plutôt que masquée : E3 garantit la **mesurabilité** du
défaut, pas son **élimination** automatique.

### D3. Métrique observable : **instrumentation runtime ET reconstruction depuis log causal**

**Voie runtime (assertion temps-réel pendant S5) :**

`InferenceQueue` maintient en mémoire une trace circulaire des
dernières N=256 requêtes admises :

```rust
struct QueueTrace {
    admission_ts: Instant,
    slot_acquired_ts: Option<Instant>,
    completion_ts: Option<Instant>,
    priority_class_at_admission: PriorityClass,
    promoted_from: Option<PriorityClass>,
    outcome: Option<InferenceOutcome>,
    agent_id: AgentId,
}
```

Exposée via `Scheduler::queue_stats() -> QueueStats { traces:
[QueueTrace; ≤256], ... }` (extension de l'`QueueStats` d'ADR-0022 D4).

Assertions S5 :

```rust
// E1 intra-classe
for class in [Supervisor, Foreground, Batch] {
    let non_promoted = traces
        .iter()
        .filter(|t| t.priority_class_at_admission == class && t.promoted_from.is_none())
        .filter(|t| t.slot_acquired_ts.is_some());
    assert_monotonic(non_promoted, |t| (t.admission_ts, t.slot_acquired_ts.unwrap()));
}

// E3 borne dure
for t in traces.iter().filter(|t| t.slot_acquired_ts.is_some()) {
    let wait = t.slot_acquired_ts.unwrap() - t.admission_ts;
    assert!(wait.as_millis() <= MAX_WAIT_MS as u128);
}
```

**Voie log causal (audit post-hoc) :**

`InferenceRequest (0x0C)` enrichi par ADR-0022 D4 porte
`priority_class`, `queue_depth_at_admission`, `promoted_from`. La
chronologie `0x0C → 0x0D/0x0E/0x0F` permet de reconstruire `wait_ms`
*partiellement* — en mesurant l'écart temporel entre `0x0C` et le
**premier message de progression côté agent** (typiquement le
`commit_barrier` qui suit le retour de `agent_infer`).

Limitation honnête : le log causal n'inscrit pas explicitement
`slot_acquired_ts`. Le moment exact de l'acquisition du slot est
**interne au scheduler** ; le `0x0C` est émis *après* acquisition
(ADR-0019 §Q3 : "Émis à l'entrée de `agent_infer` (après acquisition
du slot sémaphore)"). En conséquence :

- **`admission_ts` n'est pas dans le log.** Le `0x0C` témoigne du
  passage en `WaitingInference`, qui suit l'acquisition du slot.
- Pour rendre la voie log causal complète, il faudrait inscrire
  `admission_ts` dans le payload `0x0C`. Décision : **on n'inscrit pas
  `admission_ts` dans `0x0C` en Phase 6**. La voie runtime suffit aux
  assertions S5 ; la voie log causal sert l'audit qualitatif (a-t-on
  observé des Batch promues ? combien ? répartition par classe ?).

L'inscription d'`admission_ts` dans `0x0C` est reportée à Phase 7+ si
le besoin d'audit forensique de la latence d'attente émerge. Cohérent
avec le découpage "trace causale compacte" d'ADR-0019 §Q3.

**Pourquoi les deux voies ?** Argumenté au brief §3.2 Q-Ph6-7 :
l'instrumentation runtime garantit la testabilité déterministe ; le
log causal sert l'auditabilité production. Ces deux usages ne se
recouvrent pas — le runtime garde 256 entrées, le log les garde
toutes (modulo BlobDB sur les payloads > 4 KB, sans objet ici).

### Scénario test de référence (S5)

Le scénario `S5-fairness-priority` (Ph6-B9 du brief) doit produire les
assertions suivantes, en cohérence avec D1–D3 :

- **A-E1.** Pour les 8 agents `density_worker` (classe `Foreground`)
  spawné simultanément, l'ordre `admission_ts` croissant implique
  `slot_acquired_ts` croissant (filtre `promoted_from = None`).
- **A-E3.** Pour chaque requête servie (10 au total), `wait_time_ms ≤
  30 000`.
- **A-priorité.** Les 2 agents `supervisor_arith` (classe `Supervisor`)
  obtiennent leur slot avant les 8 `density_worker` — observable via
  l'ordre `slot_acquired_ts`.
- **A-pas-de-famine-batch.** Si la variante S5b est ajoutée (1
  supervisor + 8 batch + 4 foreground, soutenu sur 30 s), aucune
  requête batch ne reste en attente > 10 s (promotion) puis > 30 s
  (E3).

Verdict S5 = `pass` si A-E1 ∧ A-E3 ∧ A-priorité, sur 10 runs
consécutifs. A-pas-de-famine-batch est facultatif (Phase 6 partielle
acceptable selon le brief §7).

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A1. E2 (fair-share proportionnel)** | Garantie formelle de progression proportionnelle ; matches Linux CFS / stride scheduling | Demande poids et fenêtre calibrés ; pas de données en Phase 6 ; mécanique d'ADR-0022 ne le réalise pas directement | Rejetée Phase 6. Reportée Phase 7+ avec données de production. |
| **A2. E1 seul (sans E3)** | Plus simple, propriété ordinale pure | Pas de borne d'attente — un batch peut attendre indéfiniment sous flux soutenu de supervisor. Viole §C1.4–C1.5. | Rejetée. E3 indispensable. |
| **A3. E3 seul (sans E1)** | Borne d'attente sans contrainte d'ordre intra-classe | Comportement intra-classe imprévisible ; difficile à debug. Régression d'observabilité par rapport au PoC E2E. | Rejetée. E1 sert la prévisibilité. |
| **A4. `max_wait_ms = 10_000` (10 s)** | Borne plus serrée, force une discipline plus stricte | Risque de faux positifs S5 sous variabilité Tokio + backend. Pas de marge de tolérance. | Rejetée. La marge 3× au pire cas FIFO sert la robustesse des tests. |
| **A5. `max_wait_ms = 60_000` (60 s)** | Tolérance maximale | Borne si lâche qu'elle ne contraint rien (équivalent à `host_max_inference_duration_ms`). | Rejetée. Pas falsifiable. |
| **A6. Métrique log causal uniquement** | Pas d'API publique additionnelle | Nécessiterait d'inscrire `admission_ts` dans `0x0C`, surface de log élargie ; assertions S5 dépendraient de la lecture du log pendant le test | Rejetée. Coût + complexité ; instrumentation runtime suffit. |
| **A7. Métrique runtime uniquement** | Plus simple, pas besoin d'enrichir le log | Régression d'observabilité post-hoc — on ne sait plus dire dans le log si une requête était promue | Rejetée. L'enrichissement `0x0C` d'ADR-0022 D4 est déjà acquis, on l'utilise. |
| **A8. Éviction runtime sur dépassement `max_wait_ms`** | E3 devient garantie active, pas assertion test | Demande de définir le retour à l'agent (`NoSlot` ? nouveau code ?) ; pas dans la dette Phase 6 ; surface d'API. Casse D-Ph6-A si nouveau code retour. | Rejetée. Reporté Phase 7+. E3 reste assertion test en Phase 6. |
| **D1–D3 retenus** | — | — | Retenus |

---

## Conséquences

**Positives :**

- `spec/07 §C1.3, §C1.4, §C1.5` matérialisées par des assertions
  testables (A-priorité, A-pas-de-famine-batch, A-E3 dans S5).
- Définition opérationnelle sans paramètres non calibrables :
  `max_wait_ms = 30_000` et `max_starvation_ms = 10_000` sont des
  bornes lâches dérivées du pire cas analytique, pas des hypothèses.
- E1 est falsifiable trivialement par contre-exemple dans le log
  causal (pour les requêtes non promues).
- E3 est falsifiable par un seul échec de S5.
- Cohérent avec `spec/02 §4.3` (liveness conditionnelle) — E3 en est
  un raffinement chiffré.

**Négatives / coûts acceptés :**

- E2 (fair-share) explicitement reporté Phase 7+. Si une démonstration
  publique demande "fair-share", on dit qu'on a E1+E3 et que c'est
  suffisant pour les propriétés C1 en l'état des données. Pas de
  fausse profondeur.
- E3 est une **propriété d'assertion test, pas une garantie runtime
  active** en Phase 6. Si un agent demande `agent_infer` et reste en
  attente > 30 s sans être servi, son `agent_infer` ne retourne pas
  `NoSlot` à 30 s — il attend toujours. Le défaut est *observable* en
  test, pas *prévenu* en production. Documenté ; à reconsidérer
  Phase 7+ (cf. alternative A8).
- L'instrumentation runtime maintient 256 entrées en mémoire (~256 ×
  ~80 bytes = ~20 KB). Acceptable, mais à surveiller si N augmente.
- L'assertion A-E1 filtre `promoted_from = None`. Une promotion casse
  E1 par définition (la requête promue saute des voisins) ; c'est
  acceptable parce que la promotion est rare et la sortie de classe
  retire la requête de la comparaison intra-classe. C'est correct
  formellement mais demande une note explicite dans S5.

**Neutres / à surveiller :**

- Si S5 observe que >5% des requêtes Batch sont promues sous charge
  normale, c'est un signal pour Phase 7 (revisiter
  `max_starvation_ms` ou passer à E2). Inscrire en LESSONS.
- Tokio scheduler non déterministe pourrait rendre A-E1 flaky (deux
  requêtes admises dans la même milliseconde peuvent être servies
  out-of-order par le scheduler Tokio). Mitigation : `admission_ts`
  utilise un compteur monotone interne à l'`InferenceQueue` (pas
  `Instant::now()`), forçant un ordre strict même sub-ms. Cohérent
  avec Q-OPEN-Ph6-5 du brief.
- L'extension d'`admission_ts` dans `0x0C` (rejetée Phase 6) reste un
  point d'évolution Phase 7+ — pas un blocage actuel.
- Le rapport entre `max_wait_ms` et `host_max_inference_duration_ms`
  (30 s vs 60 s) est intentionnel : une requête peut attendre 30 s
  puis exécuter 60 s — soit jusqu'à 90 s total. Si une borne de bout
  en bout est requise (Phase 7+), elle se composera de ces deux
  bornes, pas par modification de l'une d'elles.

---

## Références

- ADR-0022 — File d'inférence bornée (mécanique : trois classes,
  promotion bornée à un cran, drop-newest avec éviction Batch)
- ADR-0019 — Primitive `agent_infer` (`host_max_inference_duration_ms
  = 60 000`, distinct de `max_wait_ms`)
- ADR-0014 — Politique supervision (timeout fixe 30 s, cohérent
  avec `max_wait_ms = 30 000`)
- ADR-0021 — Convention scénarios (S5 suit le patron `S<N>-<slug>`)
- ADR-0018 — `os-poc-reconstruct` (à étendre pour rendre lisible la
  chronologie d'attente — déjà couvert par Ph6-B11 du brief)
- `spec/02 §4.3` — Liveness conditionnelle (E3 en est un raffinement
  chiffré)
- `spec/07 §C1.3, §C1.4, §C1.5` — Propriétés couvertes
- `docs/archive/phase6.md §3.2` — Énoncé des questions Q-Ph6-5 à Q-Ph6-7
- [Goguen & Meseguer 1982] *Security Policies and Security Models*,
  IEEE Symp. on Security and Privacy — référence canonique pour les
  définitions de non-interférence ; non utilisé ici (E1/E3 sont des
  propriétés ordinales/temporelles, pas d'information flow), cité pour
  délimiter le scope.
- [Waldspurger 1995] — référence reportée pour E2 (Phase 7+)
- `TODO.md` D-Q-V2.6 — résolution complétée par cet ADR + ADR-0022

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
