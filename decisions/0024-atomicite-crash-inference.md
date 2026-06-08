# ADR-0024 — Atomicité crash `(InferenceCancelled, SchedulerRollback)`

**Date :** 2026-05-16
**Statut :** Acceptée

---

## Contexte

ADR-0019 §Q-V2.2 a inscrit la dette **D-Q-V2.2** : la paire d'événements
`(InferenceCancelled 0x0E, SchedulerRollback 0x0B)` émise lors d'un
rollback scheduler pendant `WaitingInference` n'est pas atomique en
Phase 2. Sur crash entre les deux émissions, `os-poc-reconstruct`
observe un `InferenceCancelled` orphelin (sans `SchedulerRollback`
postérieur dans le log).

Conséquences en Phase 2, acceptées :

- Lisibilité dégradée du log post-crash : un humain lisant le log voit
  un `0x0E` (`cause = 0x01 Rollback`) sans contexte de rollback
  scheduler — ambiguïté entre "rollback applicatif effectif" et
  "rollback initié mais non appliqué (crash)".
- `os-poc-reconstruct` peut afficher le `0x0E` mais n'a pas de
  mécanisme de réconciliation pour les `0x0E` orphelins.
- Pas d'impact fonctionnel : les agents de test ne crashent pas.

Phase 6 doit trancher la dette pour satisfaire `spec/02 §P6` (atomicité
crash). Le brief Phase 6 §3.3 énumère trois stratégies :

- **W. `WriteBatch` cross-composant** — fusionner `0x0E` et `0x0B`
  dans un seul `WriteBatch` RocksDB, garantissant atomicité par le
  WAL.
- **J. Journal de compensation** — émettre des marqueurs explicites
  (`CompensationOpen` / `CompensationClose`) encadrant la transaction
  ; détection des orphelins au recovery.
- **O. Ordre inversé + idempotence** — émettre `0x0B` avant `0x0E` ;
  un `0x0B` sans `0x0E` postérieur signifie "cancellation implicite".

Forces en présence :

- `InferencePool` (qui émet `0x0E`) et `Scheduler` (qui émet `0x0B`)
  sont des modules distincts (`poc/runtime/src/inference_pool.rs` vs
  `poc/runtime/src/lib.rs` côté scheduler). Toute solution doit
  respecter cette séparation ou justifier la fuite d'abstraction.
- ADR-0019 §Q-V2.1 a inscrit l'ordre canonique
  `cancel → send Message::Rollback → log SchedulerRollback`. Toute
  inversion casse la lisibilité documentée.
- ADR-0010 `EmitEnvelope` MessagePack supporte l'ajout d'EmitType
  (types `0x0B–0xFF` réservés explicitement). Pas de migration de
  schéma pour ajouter `0x11`/`0x12`.
- Le pattern *failpoint* (TiKV `fail-rs`, FoundationDB Buggify) est
  l'état de l'art pour tester l'atomicité crash de façon déterministe
  [TiKV fail-rs]. Adapté à un contexte Rust.

Contraintes héritées :

- **D-Ph6-A.** ABI `agent_infer` figée. Pas de nouveau code retour
  WASM. La stratégie ne doit pas modifier la sémantique vue par
  l'agent.
- **D-Ph6-F.** Les 53 tests existants restent verts. Notamment S4
  qui produit `0x0C → 0x0E → 0x0B`.
- **`spec/02 §P6`** — atomicité crash : à toute borne de commit, soit
  toutes les écritures du même commit sont visibles, soit aucune.

---

## Décision

Trois sous-décisions D1–D3.

### D1. Stratégie : **journal de compensation (option J)** avec deux nouveaux EmitType `0x11` / `0x12`

Le flux de `Scheduler::rollback` ciblant un agent en `WaitingInference`
devient :

```text
1. log(CompensationOpen 0x11) avec payload [target_agent_id u32 LE | target_seq u64 LE | initiator_agent_id u32 LE]
2. inference_pool.cancel(target_agent_id)        ← émet 0x0E InferenceCancelled normalement
3. inbox.send(Message::Rollback { target_seq })
4. log(SchedulerRollback 0x0B)                   ← inchangé ADR-0019
5. [agent consomme Message::Rollback, applique le rollback à son store]
6. log(CompensationClose 0x12) avec payload [target_agent_id u32 LE | target_seq u64 LE | outcome u8]
```

`outcome ∈ {0x00 Applied, 0x01 AgentTerminated, 0x02 AgentDisappeared}` —
permet de distinguer un rollback appliqué normalement d'une terminaison
précoce de l'agent cible.

**Définition des deux EmitType :**

| Code | Nom | Émis | Payload |
|------|-----|------|---------|
| `0x11` | `CompensationOpen` | Premier événement d'un rollback scheduler ciblé sur un agent en `WaitingInference`. Émis depuis `Scheduler::rollback` avant tout autre acte. | `[target_agent_id u32 LE \| target_seq u64 LE \| initiator_agent_id u32 LE]` (16 bytes) |
| `0x12` | `CompensationClose` | Dernier événement d'un rollback scheduler. Émis depuis `Scheduler::rollback` après application complète ou observation de terminaison. | `[target_agent_id u32 LE \| target_seq u64 LE \| outcome u8]` (13 bytes) |

`initiator_agent_id` = l'agent qui a déclenché le rollback (typiquement
un superviseur via A3 verdict `Reject`) ou `0xFFFFFFFF` si le scheduler
agit autonomement. Cohérent avec ADR-0014 (politique supervision).

**Justification du choix de J sur W et O.**

- **W (WriteBatch cross-composant) rejetée** pour la même raison
  qu'ADR-0019 §Q-V2.2 : couplage de transaction RocksDB entre
  `InferencePool` (host function `agent_infer`) et
  `Scheduler::rollback` (lib.rs). Concrètement :
  - Soit `InferencePool::cancel` retourne un `WriteBatch` non commité
    que `Scheduler::rollback` consomme et étend. Ownership cross-module
    fragile.
  - Soit `Scheduler::rollback` prépare le `WriteBatch` puis appelle
    `InferencePool::cancel_into_batch(&mut batch)`. L'`InferencePool`
    doit alors accepter de loguer dans un batch externe — fuite
    d'abstraction.
  - Soit on retarde l'émission de `0x0E` jusqu'à ce que
    `Scheduler::rollback` ait préparé `0x0B`, et on commit les deux
    ensemble. Couplage temporel : la cancellation côté Tokio est
    déjà retournée, le slot libéré, mais le `0x0E` n'est pas inscrit
    avant ~quelques µs plus tard. Risque de "Future already returned
    but not logged" si crash dans cet intervalle — déplace le bug,
    ne le résout pas.

  Coût total estimé : ~300–500 LoC modifiées dans `InferencePool` +
  `Scheduler`, surface de tests doublée, refonte d'API. Dépasse le
  seuil 500 LoC mentionné au brief §7 ("à reconsidérer si refonte
  majeure").

- **O (ordre inversé) rejetée** parce qu'elle viole la lisibilité de
  la séquence canonique d'ADR-0019 §Q-V2.1. Un humain lisant
  `0x0B → 0x0E` est forcé de raisonner inversement à la sémantique
  ("le rollback est annoncé avant la cancellation, donc..."). De plus,
  un `0x0B` sans `0x0E` postérieur devient sémantiquement ambigu :
  est-ce "cancellation implicite" ou "agent n'était pas en
  `WaitingInference` au moment du rollback" (cas légitime existant) ?

- **J (journal de compensation) retenue** pour trois raisons :
  - Découplage préservé : `InferencePool` continue d'émettre `0x0E`
    directement, sans connaître l'existence de `Scheduler`. Le
    `CompensationOpen`/`Close` est strictement du ressort du
    `Scheduler`.
  - Auditabilité forte : la paire `(0x11, 0x12)` est un marqueur
    transactionnel explicite, lisible humainement et machinalement.
  - Atomicité par détection : un `0x11` sans `0x12` correspondant
    *est* la signature de crash. La récupération est explicite, pas
    implicite.

**Coût de la stratégie J.**

- 2 entrées de log additionnelles par rollback scheduler ciblé sur
  un agent en `WaitingInference`. Payload total : 16 + 13 = 29 bytes.
  Négligeable face au header MessagePack (~30 bytes par entrée).
- 2 `append` RocksDB additionnels (~20 µs par fsync sur NVMe local
  sous WAL synchrone). Acceptable.
- Politique de réconciliation à implémenter dans `os-poc-reconstruct`
  (cf. D3).

**Atomicité partielle conservée.** La paire `(0x0E, 0x0B)` reste
non-atomique au sens strict — un crash peut toujours laisser un `0x0E`
sans `0x0B`. Mais la paire `(0x11, 0x12)` est désormais le **marqueur
transactionnel** : si `0x11` est inscrit mais `0x12` absent, on sait
que la transaction n'a pas abouti et on applique D3. La granularité
d'atomicité passe de "deux EmitType" à "un bloc transactionnel
explicite".

### D2. Mécanisme de test : trait `CrashPoint` feature-gated

Pattern *failpoint* inspiré de TiKV `fail-rs` (cf. brief §10).

```rust
#[cfg(feature = "crash-injection")]
pub trait CrashPoint: Send + Sync {
    fn fire(&self, name: &'static str);
}

#[cfg(not(feature = "crash-injection"))]
pub trait CrashPoint: Send + Sync {
    #[inline(always)]
    fn fire(&self, _name: &'static str) {}
}
```

L'implémentation production est un no-op inlined ; l'implémentation
test (`TestCrashPoint`) vérifie le nom et appelle `std::process::exit(0)`
ou `panic!()` selon configuration.

**Points d'injection nommés dans `Scheduler::rollback` :**

| Nom | Position | Sert à tester |
|-----|----------|---------------|
| `"rollback.before_compensation_open"` | Avant émission `0x11` | Cas pré-transaction — aucune trace. Recovery doit ignorer (rien à réconcilier). |
| `"rollback.after_compensation_open"` | Entre `0x11` et `inference_pool.cancel()` | `0x11` orphelin, pas de `0x0E`, pas de `0x0B`. Recovery détecte. |
| `"rollback.between_cancel_and_send"` | Entre `cancel()` et `inbox.send(Rollback)` | `0x11` + `0x0E` orphelins, pas de `0x0B`. Recovery détecte. |
| `"rollback.between_send_and_log_b"` | Entre `inbox.send(Rollback)` et `log(0x0B)` | `0x11` + `0x0E`, pas de `0x0B`, message en inbox non consommé (perdu au crash). Recovery détecte. |
| `"rollback.between_log_b_and_compensation_close"` | Entre `0x0B` et `0x12` | Tous présents sauf `0x12`. Recovery détecte transaction "presque close". |

**Build :** feature `crash-injection` désactivée par défaut. Le binaire
release `cargo build --release` ne contient pas le symbole. Vérification
inscrite dans `Cargo.toml` :

```toml
[features]
default = []
crash-injection = []  # uniquement activable explicitement pour tests
```

Test de non-régression du binaire release :

```bash
# Vérifier que le binaire release ne contient pas de point d'injection
strings target/release/poc-runtime | grep -c "rollback\." || echo "OK: aucun point d'injection en release"
```

Inscrit dans `scenarios/run-all.sh` (vérification de release).

### D3. Politique de réconciliation au recovery dans `os-poc-reconstruct`

À l'ouverture du log par `os-poc-reconstruct`, **après lecture
complète** du log :

1. Constituer la liste des paires `0x11 / 0x12` par
   `(target_agent_id, target_seq)`. Un `0x12` se rattache au plus
   récent `0x11` non encore clos pour le même couple.
2. Pour chaque `0x11` sans `0x12` correspondant :
   a. Lister les événements postérieurs au `0x11` jusqu'au prochain
      `0x11/0x12` ou la fin du log : `0x0E` ? `0x0B` ?
   b. Classer la transaction :
      - `0x11` seul → "compensation initiée, aucun acte" (faible
        impact).
      - `0x11 + 0x0E` → "cancellation effectuée, rollback non
        envoyé" (l'agent revenu de `Cancelled (4)` peut avoir
        progressé arbitrairement).
      - `0x11 + 0x0E + 0x0B` → "rollback diffusé, application
        inconnue" (l'agent peut avoir appliqué ou pas selon timing
        crash).
   c. **Politique par défaut : auto-close + warning.** Une entrée
      synthétique `[reconciled: auto-close, classification: ..., at:
      recovery_ts]` est ajoutée à la sortie humaine de
      `os-poc-reconstruct` (pas écrite dans le log RocksDB — log
      append-only inviolé). Avertissement émis sur stderr.
   d. **Politique alternative : manual-review.** Configurable via
      flag `os-poc-reconstruct --on-orphan=halt` qui termine avec
      exit code non-nul et exige une revue humaine. Utile en CI pour
      détecter des crashes non attendus.

3. Pour chaque `0x12` sans `0x11` correspondant : entrée
   pathologique, signalée comme corruption probable. Politique :
   `halt` systématique (exit non-nul). Ne devrait jamais arriver en
   pratique (un `0x12` requiert un `0x11` antérieur émis par le même
   `Scheduler::rollback`).

**`os-poc-reconstruct` reste read-only.** La réconciliation est
**présentationnelle**, pas correctrice. Le log RocksDB n'est jamais
modifié post-écriture (cohérent avec ADR-0018 et le principe
append-only du log causal).

**Limite assumée.** En cas de classification `0x11 + 0x0E + 0x0B`
(rollback diffusé, application inconnue), `os-poc-reconstruct` ne
peut pas déterminer si l'agent cible avait commencé à appliquer le
rollback avant le crash. Le `ContentStore` de l'agent au recovery
reflète l'état réel, mais le log n'enregistre pas les actions
applicatives du rollback (le rollback n'incrémente pas `seq`,
ADR-0019 §Q7). En pratique, un nouveau `Scheduler` au redémarrage
peut reconstruire l'état effectif à partir du `ContentStore` (qui est
authoritative pour les données) ; le log permet seulement de tracer
la **causalité** de la transition.

---

## Alternatives considérées

| Alternative | Avantages | Inconvénients | Raison du rejet |
|-------------|-----------|---------------|-----------------|
| **A1. W — WriteBatch cross-composant** | Atomicité forte garantie par RocksDB WAL ; pas d'EmitType supplémentaire | Couplage `InferencePool ↔ Scheduler` ; refonte d'API ; ~300–500 LoC ; risque de "Future returned but not logged" déplaçant le bug | Rejetée. Coût > seuil refonte majeure du brief §7. |
| **A2. O — Ordre inversé + idempotence** | Pas de WriteBatch, pas d'EmitType supplémentaire | Casse la lisibilité ADR-0019 §Q-V2.1 ; `0x0B` orphelin devient ambigu (cas légitime existant : rollback sur agent non en WaitingInference) | Rejetée. Ambiguïté sémantique inacceptable. |
| **A3. Accepter la dette permanente (status quo)** | Coût nul | Viole `spec/02 §P6` en présence de crashes ; observabilité dégradée | Rejetée. Ph6-B7 du brief demande explicitement résolution. |
| **A4. WriteBatch interne au Scheduler** (`Scheduler::rollback` accumule `0x0E` et `0x0B` dans un batch local et commit ensemble) | Découplage `InferencePool` préservé | Demande de retarder `0x0E` jusqu'à `Scheduler::rollback`, ce qui contredit ADR-0019 §Q-V2.1 (cancellation immédiate trace `0x0E`). Casse la séquence d'observation côté agent (cancellation visible avant que le slot soit effectivement libéré côté log). | Rejetée. Inversion temporelle log/runtime. |
| **A5. Idempotence + replay** (au recovery, rejouer la séquence depuis le `0x0E` ; l'agent re-traite le `Rollback`) | Réconciliation active, pas seulement signalée | Demande de re-spawn l'agent dans son état pré-crash, replay des messages — complexité élevée ; ADR-0006/0009 ne couvrent pas le replay. Phase 6 hors scope (cf. brief §5.3 "pas de récupération multi-noeud"). | Rejetée. Reporté Phase 7+. |
| **A6. Failpoint via `kill -9` externe** | Pas de code dans le runtime | Non déterministe ; synchronisation point précis impossible sans IPC supplémentaire | Rejetée. Le brief §3.3 Q-Ph6-10 préfère le failpoint instrumenté (option a). |
| **D1–D3 retenus** | — | — | Retenus |

---

## Conséquences

**Positives :**

- D-Q-V2.2 résolue : `os-poc-reconstruct` peut détecter et classer
  toute transaction de rollback incomplète.
- `spec/02 §P6` partiellement adressée pour la transition
  `(InferenceCancelled, SchedulerRollback)`. SEF-4 général (toute
  commit barrier) reste séparé.
- Découplage `InferencePool ↔ Scheduler` préservé. Pas de refonte
  d'API, pas de fuite d'abstraction.
- ABI WASM inchangée. Agent ne voit aucune différence (cohérent
  D-Ph6-A).
- Failpoint pattern reproductible — tests crash déterministes,
  exécutables en CI sans tooling externe.
- Auditabilité forte : un opérateur lisant le log via
  `os-poc-reconstruct` voit explicitement quelles transactions ont
  été interrompues et avec quel niveau d'avancement.

**Négatives / coûts acceptés :**

- 2 EmitType supplémentaires (`0x11`, `0x12`). Étendent l'enum
  `EmitType` et `TryFrom<u8>` dans `poc/causal-log/src/lib.rs`.
  Mineur.
- 2 `append` RocksDB additionnels par rollback scheduler ciblé sur
  agent en `WaitingInference`. Latence ~40 µs additionnels sur NVMe
  local. Négligeable face à la durée typique d'une inférence (~2,5 s).
- Politique de réconciliation `os-poc-reconstruct` à coder (Ph6-B11
  partiel du brief). Surface ~100 LoC.
- L'atomicité reste *par détection*, pas *par garantie*. Un `0x0E`
  émis sans `0x11` antérieur reste une régression d'invariant — à
  inscrire en assertion forte de `Scheduler::rollback` : aucune
  émission de `0x0E` n'est attendue dans un chemin de rollback hors
  de la transaction `0x11/0x12`.
- Test crash via `std::process::exit(0)` ne nettoie pas les
  ressources Tokio. C'est intentionnel — c'est le comportement
  testé (crash sale). Tokio runtime au redémarrage est neuf.
- La feature `crash-injection` doit être strictement gardée : un
  release qui embarquerait le symbole `rollback.*` serait un risque
  d'exfiltration ou de panic injecté en production. Vérification
  inscrite dans `run-all.sh`.

**Neutres / à surveiller :**

- Si Phase 7+ introduit le replay actif (A5), la stratégie J reste
  compatible — le `0x11/0x12` devient le pivot de la transaction
  rejouable.
- Si une autre transition future demande l'atomicité crash similaire
  (e.g. `(InferenceFailed, ScheduledRetry)` Phase 7+), le pattern
  `CompensationOpen/Close` peut être réutilisé avec d'autres
  payloads. Pas d'engagement préventif en Phase 6.
- Le test S6 `crash-during-cancel` (Ph6-B10) est marqué optionnel
  Semaine 4 du brief. Si Semaine 2 glisse, S6 est reporté Phase 7+ ;
  les tests unitaires `t_crash_*` couvrent la majeure partie de la
  garantie sans S6.
- `os-poc-reconstruct --on-orphan=halt` peut être activé en CI pour
  durcir la détection. Recommandation provisoire : `auto-close +
  warning` par défaut (compatibilité avec tests existants), `halt`
  pour les tests S6 dédiés.

---

## Références

- ADR-0019 — Primitive `agent_infer` (séquence canonique
  `0x0C → 0x0E → 0x0B`, §Q-V2.1 ordre cancel→send→log, §Q-V2.2
  dette D-Q-V2.2 résolue par cet ADR)
- ADR-0010 — Contrat `emit` (EmitType, format MessagePack — autorise
  les types `0x0B–0xFF` réservés ; `0x11`/`0x12` s'inscrivent)
- ADR-0011 — Options RocksDB (WAL synchrone, CF `default` — pas
  d'impact migration)
- ADR-0014 — Politique supervision (timeout, no retry — cohérent ;
  `initiator_agent_id` permet de tracer le déclencheur de rollback)
- ADR-0018 — `os-poc-reconstruct` (à étendre pour rendre lisibles
  `0x11`/`0x12` et appliquer la politique de réconciliation D3 ;
  Ph6-B11 du brief)
- ADR-0022 — File d'inférence bornée (les éviction `NoSlot`
  d'ADR-0022 D2 ne déclenchent pas le pattern `0x11/0x12` —
  l'éviction n'est pas un rollback)
- `spec/02 §P6` — Atomicité crash (partiellement adressée par cet ADR
  pour la transition `(0x0E, 0x0B)`)
- `docs/archive/phase6.md §3.3` — Énoncé des questions Q-Ph6-8 à Q-Ph6-10
- `TODO.md` D-Q-V2.2 — Dette résolue par cet ADR
- TiKV `fail-rs` — https://github.com/tikv/fail-rs — pattern failpoint
  référence
- FoundationDB Buggify — https://apple.github.io/foundationdb/buggify.html
  — pattern d'injection de fautes pour tests d'atomicité
- [Gray & Reuter 1992] *Transaction Processing: Concepts and
  Techniques* — chapitre sur les compensations transactionnelles (saga
  pattern) ; le journal de compensation D1 est une variante simplifiée

---

*Format inspiré de MADR — [Nygard 2011] "Documenting Architecture Decisions"*
