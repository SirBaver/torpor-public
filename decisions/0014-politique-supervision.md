# ADR-0014 — Politique de supervision : timeout, watchdog, retry, escalade

**Date :** 2026-05-14
**Statut :** Acceptée (2026-05-14 — promotion depuis stub suite au premier cas d'usage concret : `ValidationVerdict::Timeout` jamais émis)

---

## Amendement 2026-06-07 (M2, revue sécurité) — `Terminated` absorbant + court-circuit de cycle

La revue sécurité a montré que `agent_terminate` (host fn, A4) ne court-circuitait **pas** le cycle
WASM : après l'appel, le guest pouvait continuer à exécuter du code et appeler d'autres host fns
mutantes (`emit`, `commit_barrier`, `agent_add_cause`, `agent_request_validation`,
`agent_checkpoint`, `agent_self_rollback`). Pire, `agent_request_validation` réécrivait
`lifecycle = AwaitingValidation`, **écrasant** `Terminated` → l'agent était « ressuscité » et
bloqué en attente de verdict au lieu de terminer. Vecteurs : actions « post-mortem » dans le log
(log-spoofing), faux `Terminated` suivi de poursuite, évasion de la terminaison auto-demandée.

**Décision (contrat de terminaison côté guest) :**

1. **`Terminated` est un état ABSORBANT.** `log_lifecycle_event` refuse toute transition sortante
   de `Terminated` (cohérent spec A4 : « un agent terminé ne se respawn pas »).
   `record_validation_response` ne ramène pas un agent terminé à `Active`.
2. **`agent_terminate` court-circuite le cycle** via un drapeau `AgentState.termination_requested`
   posé avant le log. Toute host fn mutante consulte ce drapeau en entrée et devient **no-op**
   (retour du code d'erreur conventionnel : `-3`/`-1` selon la fn ; `()` pour emit/commit_barrier)
   — aucun effet n'est produit après la demande de terminaison dans le même cycle WASM. On ne peut
   pas interrompre le WASM en plein host call (hors `epoch_interruption`) ; le no-op en entrée est
   la voie propre. `run_loop` continue de tester `Terminated` entre messages (inchangé).

Les lectures (`agent_introspect`, `agent_get_verdict`, `agent_session_info`) restent permises après
terminaison (sans effet). `agent_store_put` post-mortem n'est pas gardé (KV privé-par-agent, sur le
point d'être détruit ; effet nul sur le log hormis un éventuel `0x14` rate-limité — résiduel mineur
tracé). Test : `m2_terminate_is_absorbing_no_post_mortem_effects` (`poc/runtime/src/lib.rs`). Code :
`poc/runtime/src/actor.rs` (`termination_requested`, gardes des host fns, `log_lifecycle_event`
absorbant, `record_validation_response`). Réf. spec A4 (`02c-primitives-agent.md`).

---

## Contexte

ADR-0013 (2026-05-14) acte deux choses qui rendent la rédaction du présent stub nécessaire :

1. **D2 — concentration d'autorité dans le Scheduler** : la décomposition `Registry / Dispatcher / Supervisor / Spawn` est différée à Phase 3+ avec un critère de déclenchement explicite — « la décomposition devient obligatoire dès qu'une politique de supervision ajoute une logique conditionnelle dans une méthode existante du `Scheduler` au-delà du simple routage de message ». Le déclencheur est nommé : *toute* introduction d'une politique conditionnelle (timeout, watchdog, retry, restart) doit être précédée de la rédaction d'ADR-0014.
2. **D1 — séparation `Suspended` / `AwaitingValidation`** : la primitive A3 (`agent_request_validation`) place l'agent en `AwaitingValidation` et attend un `Message::ValidationResponse`. La boucle interne (`poc/runtime/src/actor.rs` ~ligne 1009) ignore tout autre message. **Aucun mécanisme actuel ne déclenche un verdict si le superviseur humain ne répond pas** — un agent en `AwaitingValidation` peut rester bloqué indéfiniment. Le code prévoit déjà la variante `ValidationVerdict::Timeout` (actor.rs ~ligne 1021) mais aucune logique ne l'émet. C'est exactement le premier cas d'usage de politique conditionnelle anticipé.

Le présent ADR **réserve le numéro 0014, fige le scope figé des 4 sous-questions, et liste les options connues**. Il **ne tranche aucune décision**. La rédaction de la décision elle-même est différée à la phase où le premier cas d'usage concret est identifié — pour éviter de spécifier une politique sans données ni besoin contraint.

### Pourquoi un stub plutôt qu'attendre

- Réserver le numéro évite la collision avec d'autres ADRs en cours (ADR-0015 déjà cité dans ADR-0013 §D3 pour le cas « propagation d'erreur cross-agent »).
- Figer le scope évite la dérive : sans périmètre écrit, la première PR qui introduit un timeout risque d'embarquer du retry et de l'escalade par cohérence apparente, alors que ces dimensions ont des coûts et des dépendances distincts.
- Lister les options connues *aujourd'hui* documente l'état de l'art que nous avons sous la main au moment où le stub est écrit, sans préempter la décision finale.

### Pourquoi *pas* de décision maintenant

- Aucun workload réel n'a encore mesuré la fréquence des cas où un agent reste en `AwaitingValidation` au-delà d'une borne raisonnable. Choisir une valeur de timeout sans cette mesure produit un nombre arbitraire (cf. critique des constantes magiques type `30s`).
- Le mécanisme de watchdog dépend du substrat choisi (Phase 2 = Tokio runtime sur Linux ; Phase 4+ = à décider). Spécifier un watchdog avant le substrat lierait la décision à un détail d'implémentation transitoire.
- Le critère de réussite d'un retry (que veut-on protéger ?) n'est pas formulé. Sans cible, toute politique de retry est de la décoration.

---

## Scope figé

ADR-0014 (lorsque rédigé en version Acceptée) couvre **exactement** les quatre sous-questions suivantes, et rien d'autre :

### (a) Politique de timeout sur `AwaitingValidation`

**Question :** au bout de combien de temps (ou selon quel prédicat) un agent en `AwaitingValidation` reçoit-il automatiquement un `Message::ValidationResponse` avec `verdict = Timeout` ?

**Sous-questions :**
- Forme du timeout : valeur fixe globale ? Valeur par `risk_level` (cf. payload `ValidationRequest`) ? Fonction paramétrée du contexte (par exemple : `min(global_max, session_remaining_budget)`) ?
- Origine de la valeur : configuration au `Scheduler::new` ? Au spawn de l'agent ? Encodé dans la capability de validation ?
- Reset du timer : un retry de `ValidationRequest` réinitialise-t-il le timer ? Ou la mesure est-elle absolue depuis la première demande de la session ?

**Hors-scope :** la politique de timeout sur les *autres* états d'attente (par exemple `AwaitingExternalIO` si introduit en Phase 3). Si une politique unifiée émerge, elle fera l'objet d'un ADR séparé.

### (b) Watchdog : qui détecte quoi

**Question :** quel composant scrute les agents en attente et déclenche la sortie de timeout ?

**Sous-questions :**
- Composant : tâche Tokio interne au `Scheduler` ? Acteur dédié `WatchdogActor` (consommerait son propre `AgentId`, log causal, etc.) ? Superviseur humain externe via API REST de poll ? Combinaison ?
- Granularité de scrutation : par agent (un timer par agent en `AwaitingValidation`) ? Par scan périodique de tous les états d'agent ? Hybride (scan grossier + timer fin par agent au-delà d'un seuil) ?
- Cible de détection : uniquement `AwaitingValidation` ? Aussi `Active` sans progression (`seq` stagnant) ? Aussi `Suspended` au-delà d'une borne (A4 ne devrait pas avoir de timeout — décision humaine — mais on peut vouloir l'observer) ?

**Hors-scope :** la détection de crashs *processus* (kill -9, panic Rust) — relève de P6 (atomicité crash) et de la recovery, pas de la supervision.

### (c) Retry : qui décide, sur quel signal

**Question :** quand un verdict `Timeout` (ou `Reject`) est émis, qu'est-ce qui se passe ensuite ?

**Sous-questions :**
- Politique par défaut : pas de retry automatique (verdict est terminal côté Scheduler — c'est l'agent qui re-demande s'il veut) ? Re-issue automatique de `ValidationRequest` ? Re-issue conditionnelle (par exemple seulement si `verdict = Timeout`, pas si `Reject`) ?
- Décideur : le `Scheduler` (politique uniforme), l'agent (logique applicative dans son code WASM), une capability `cap_retry` (l'agent peut retry seulement s'il la possède) ?
- Backoff : exponential ? Fixe ? Pas de retry (n=0) ?
- Limite : nombre max de retries par session (ADR-0012) ? Par action ? Pas de limite (responsabilité agent) ?

**Hors-scope :** le retry au niveau message (re-livraison après échec de delivery) — relève du dispatcher de messages, pas de la supervision.

### (d) Escalade : vers qui

**Question :** quand un agent ne peut plus progresser (timeouts répétés, refus répétés, état incohérent détecté par watchdog), à qui l'événement est-il notifié ?

**Sous-questions :**
- Destinataire : superviseur humain via inbox dédiée (acteur `human_supervisor` avec `AgentId` réservé) ? Agent parent dans la hiérarchie causale (`parent_cause` issu de `spawn_child`) — explicitement *rejeté* par ADR-0013 §D3, donc non-option ? Capability d'escalade pré-allouée à un agent superviseur applicatif ?
- Forme du message d'escalade : nouvelle variante `EmitType::Escalation` dans le log causal (cf. ADR-0010) ? Message hors-bande via canal `Scheduler::escalate` ?
- Action consécutive : suspension automatique de l'agent en faute (passage à `Suspended` A4) ? Pas d'action automatique (notification seule) ?

**Hors-scope :** la *politique* du destinataire de l'escalade (que fait le superviseur humain en réponse). Cela relève de l'application, pas du substrat.

---

## Options connues à ce jour

Cette section liste les options qui ont émergé dans les discussions à 2026-05-14, sans en privilégier aucune. Elle sera affinée lorsque le premier cas d'usage déclenchera la promotion de cet ADR au statut « Acceptée ».

### Options pour (a) timeout

| Option | Avantages probables | Inconvénients probables |
|--------|---------------------|--------------------------|
| Valeur fixe globale (par exemple 30 s) | Simple ; testable ; pas de configuration runtime | Arbitraire ; uniformise des cas hétérogènes (risk_level=low vs critical) |
| Par `risk_level` | Suit la sémantique déjà encodée dans le payload `ValidationRequest` | Triple la surface de configuration ; valeur de chaque palier reste arbitraire |
| Fonction du budget de session restant (ADR-0012) | Lie naturellement timeout à la borne de session | Couple deux mécanismes ; un bug dans le calcul de budget casse le timeout |
| Encodé dans la capability `cap_request_validation` | Atténuation par capability (le délégant fixe le timeout du délégué) | Demande extension du modèle capability ; pas de précédent dans ADR-0005 |

### Options pour (b) watchdog

| Option | Avantages probables | Inconvénients probables |
|--------|---------------------|--------------------------|
| Tâche Tokio interne au `Scheduler` (timer par agent) | Léger ; pas de nouvel acteur ; latence détection ≈ tick timer | Concentre encore plus d'autorité dans `Scheduler` — déclencherait précisément la décomposition de D2 |
| Acteur dédié `WatchdogActor` | Décomposition propre (mécanisme/politique) ; testable indépendamment | Coût IPC ; latence détection plus élevée ; soulève question : qui supervise le watchdog ? |
| Scan périodique de tous les agents (sans timer par agent) | Simple ; O(N) par tick prévisible | Latence détection = période de scan (granularité grossière) ; scale mal au-delà de ~10⁴ agents |
| Hybride : scan grossier + timer fin pour les agents proches de la limite | Compromis latence/coût | Complexité ; deux mécanismes à raisonner ensemble |
| Pas de watchdog actif — poll par superviseur humain via REST | Aucun coût runtime | Latence détection arbitraire ; déplace le problème au client |

### Options pour (c) retry

| Option | Avantages probables | Inconvénients probables |
|--------|---------------------|--------------------------|
| Aucun retry automatique (verdict terminal côté Scheduler) | Minimaliste ; responsabilité côté agent ; aucun mécanisme caché | Force chaque agent à coder son propre retry ; risque de duplication non-uniforme |
| Re-issue conditionnel (sur `Timeout`, pas sur `Reject`) | Distinction sémantique pertinente (Timeout = absence de réponse, Reject = décision) | Nécessite distinguer dans le payload de verdict ; encore une politique à fixer |
| Délégation via capability `cap_retry` | Atténuation par capability ; agents non-prioritaires sans `cap_retry` ne peuvent pas spammer | Demande extension du modèle capability |
| Backoff exponential automatique avec limite (par exemple 3 retries × 2^n) | Pattern classique ; éprouvé sur retries d'IO | Constantes magiques ; le contexte « validation humaine » n'est pas un IO transient |

### Options pour (d) escalade

| Option | Avantages probables | Inconvénients probables |
|--------|---------------------|--------------------------|
| Inbox dédiée à un `human_supervisor` (AgentId réservé) | Simple ; réutilise le canal Message existant | Réserve un AgentId magique ; couplage statique au modèle humain-au-clavier |
| Capability `cap_escalate_to(target)` pré-allouée au spawn | Atténuation par capability ; flexibilité du destinataire | Demande spec de pré-allocation ; quel agent l'alloue ? |
| Nouvelle variante `EmitType::Escalation` dans le log causal | Audit naturel ; le superviseur humain consulte le log (cohérent avec modèle A d'ADR-0006) | Push vs pull — l'escalade reste passive tant que personne ne lit le log |
| Pas d'escalade — agent en `AwaitingValidation` indéfini = surveiller via poll/monitoring externe | Aucun mécanisme à concevoir | Régression fonctionnelle ; le timeout sans escalade ne sert qu'à terminer l'agent |

---

## Critère de déclenchement

ADR-0014 passe du statut **En attente** au statut **Proposée** dès qu'au moins une des conditions suivantes est satisfaite :

1. Une PR introduit dans `Scheduler::*` (`poc/runtime/src/scheduler.rs`) une logique conditionnelle qui teste une condition d'attente, un délai, un compteur de tentatives, ou une transition d'état non triviale au-delà du simple routage de message — cf. ADR-0013 §D2. **Toute PR de cette nature doit être bloquée tant qu'ADR-0014 n'est pas Proposée.**
2. Un cas d'usage concret est documenté dans `lab/LESSONS.md` montrant qu'un agent en `AwaitingValidation` est resté bloqué dans un scénario expérimental, et qu'une mesure de durée moyenne / max de blocage existe.
3. La spec passe en Phase 3 et déclare une exigence formelle de liveness conditionnelle (cf. `spec/02-properties.md` §4.3 — la propriété adressée à la place de la liveness absolue est explicitement laissée en attente).

ADR-0014 passe du statut **Proposée** au statut **Acceptée** lorsque les quatre sous-questions (a)-(d) sont tranchées, chacune avec une option retenue et une justification.

---

## Décision

Promotion stub → Acceptée déclenchée par la condition 1 du critère de déclenchement : une PR introduit dans la couche supervision une logique conditionnelle (test de timeout sur `AwaitingValidation`) qui n'existait pas auparavant. Cette PR encode `ValidationVerdict::Timeout` (déjà déclaré dans `actor.rs` mais jamais émis) comme un verdict effectivement produit.

Les quatre sous-questions sont tranchées comme suit. Toutes les décisions sont **minimales pour Phase 2**, **falsifiables**, et **strictement à l'intérieur des contraintes D2 et D3 d'ADR-0013** (pas de logique de politique ajoutée à `Scheduler::*`).

### D14.a — Timeout : valeur fixe configurable par agent, push depuis `run_loop`

**Décidé :**
- Champ `validation_timeout_ms: u64` ajouté à `AgentState`, valeur par défaut `30_000` ms (30 s).
- Origine de la valeur : configurée à la construction de l'`ActorInstance` (variant `new_precompiled_with_caps_and_timeout`, ou via un setter explicite avant `register`) ; les constructeurs existants conservent la valeur par défaut.
- Forme : valeur **unique par agent**, ni paramétrée par `risk_level`, ni couplée au budget de session (ADR-0012). Justification : aucune donnée ne permet aujourd'hui de calibrer une variation par `risk_level` ; un couplage avec le budget de session ajouterait une dépendance bidirectionnelle entre deux mécanismes orthogonaux.
- Reset du timer : la mesure est **absolue depuis l'entrée en `AwaitingValidation`**. Aucune ré-issue de `ValidationRequest` n'est gérée par le substrat en Phase 2 (cf. D14.c). Si l'agent re-demande une validation après `Timeout`, c'est un nouveau cycle, donc un nouveau timer.

**Justification de la valeur par défaut `30_000` ms :**
Cette valeur est **provisoire** et explicitement **non-mesurée**. Elle est choisie comme un ordre de grandeur défensif : assez long pour qu'un superviseur humain en réponse synchrone (REST poll, UI) ait le temps de répondre, assez court pour qu'un agent abandonné ne reste pas piégé indéfiniment dans un scénario de test. Une mesure réelle viendra avec Phase 3 (workload réel) et une révision sera ouverte si la médiane observée des réponses dépasse 30 s ou si la queue P99 indique une borne plus pertinente. Cette valeur est marquée comme `SESSION_DEFAULT_VALIDATION_TIMEOUT_MS` pour signaler son caractère par-défaut révisable.

**Ce qui n'est pas dans le scope D14.a :**
- Paramétrage par `risk_level` — reporté à un éventuel ADR de révision si les données le justifient.
- Encodage dans une capability `cap_request_validation` — pas de précédent dans ADR-0005, reporté.

### D14.b — Watchdog : `tokio::time::timeout` dans `run_loop`, pas d'acteur dédié

**Décidé :**
- La boucle d'attente `inbox.recv().await` interne à `run_loop` (actor.rs ~ligne 1009-1024) est encapsulée dans `tokio::time::timeout(Duration::from_millis(validation_timeout_ms), inbox.recv())`.
- À l'expiration : `run_loop` lui-même appelle `record_validation_response(ValidationVerdict::Timeout)` directement, sans router le verdict via une `Message::ValidationResponse` synthétique. Justification : éviter la complication d'un message « auto-envoyé » qui exigerait un sender vers soi-même ; le point de décision est local à l'agent.
- Granularité : **un timer par agent en `AwaitingValidation`**, démarré uniquement à l'entrée dans cet état, annulé naturellement à la réception d'une `ValidationResponse` (le `tokio::time::timeout` se résout à `Ok(...)`).
- Cible de détection : **uniquement `AwaitingValidation`**. Pas de surveillance sur `Active` (stagnation `seq`) ni sur `Suspended` (A4 = décision humaine, hors-scope explicitement, cf. ADR-0013 §D1).

**Conformité avec ADR-0013 §D2 :**
La logique conditionnelle vit dans `run_loop` (`poc/runtime/src/actor.rs`), **pas dans `Scheduler::*`**. Le critère de déclenchement de la décomposition `Registry/Dispatcher/Supervisor/Spawn` (ADR-0013 §D2) cite explicitement « une méthode existante du `Scheduler` ». `run_loop` est une fonction libre du module `actor`, hors de cette portée. La décomposition n'est donc **pas** déclenchée par la présente décision. Cette frontière sera réexaminée si une politique de timeout future doit être centralisée au scheduler (par ex. pour quotas globaux).

**Ce qui n'est pas dans le scope D14.b :**
- `WatchdogActor` dédié — reporté ; surcoût IPC injustifié en Phase 2.
- Scan périodique global — reporté ; pertinent au-delà de ~10⁴ agents, pas de cible Phase 2.
- Hybride scan + timer fin — reporté.

### D14.c — Retry : aucun retry automatique, verdict `Timeout` terminal côté substrat

**Décidé :**
- Le verdict `Timeout` est délivré à l'agent **comme n'importe quel autre verdict**. La transition de `AwaitingValidation` → `Active` se fait dans `record_validation_response` indépendamment du verdict reçu.
- Aucune re-issue automatique de `ValidationRequest` par le substrat. **Si un agent veut retry, il code la logique dans son WASM** : lecture du verdict via `agent_get_verdict()`, branchement sur `Timeout`, nouvel appel `agent_request_validation()`.
- Aucune limite globale de retries imposée par le substrat. Les bornes existantes restent : `SESSION_DEFAULT_MAX_ACTIONS` (ADR-0012) plafonne le nombre d'actions par session ; chaque tentative consomme une action.

**Justification :**
- Cohérent avec D3 d'ADR-0013 (pas de politique opérationnelle pré-cuite côté substrat).
- Cohérent avec le principe de séparation mécanisme/politique (seL4 [Klein et al. 2009]) : le substrat fournit le mécanisme (`Timeout` observable, primitive `request_validation` disponible), la politique (réessayer ou abandonner) appartient à l'agent.
- Le coût d'absence de retry est borné : un agent buggé qui boucle indéfiniment sera arrêté par `SESSION_DEFAULT_MAX_ACTIONS` (10 000 actions). Un agent bien écrit branche son comportement explicitement.

**Ce qui n'est pas dans le scope D14.c :**
- `cap_retry` — reporté ; demande extension d'ADR-0005.
- Backoff exponential intégré — rejeté ; pattern IO transient inadapté à la validation humaine.

### D14.d — Escalade : observation via verdict `Timeout` dans le log causal, pas de nouvel `EmitType` en Phase 2

**Décidé :**
- L'événement « un agent a subi un timeout de validation » est observable via le log causal en filtrant les entrées `EmitType::ValidationResponse` (ADR-0010) dont le payload contient `verdict == ValidationVerdict::Timeout` (octet `2`).
- **Aucun nouvel `EmitType::Escalation` n'est introduit en Phase 2.** Modifier `EmitType` (ADR-0010) sans cas d'usage qui démontre l'insuffisance du filtrage actuel reviendrait à étendre le contrat de log par anticipation.
- Aucune inbox `human_supervisor` n'est instaurée. Aucune capability `cap_escalate_to` n'est pré-allouée. Le destinataire de l'escalade est *par défaut tout lecteur du log causal* (cohérent avec ADR-0006 modèle A : supervision = lecture continue du log).
- Aucune action automatique consécutive (pas de transition `Suspended` automatique, pas de termination). L'agent reçoit le verdict `Timeout`, reprend `Active`, et son code WASM décide.

**Justification :**
- Le filtrage `verdict == Timeout` répond à la question « quels agents ont subi un timeout ? » en lookup O(N_response_entries) — acceptable en Phase 2 sans cas d'usage qui le mettrait en tension.
- Cohérent avec D3 d'ADR-0013 : la politique du destinataire (que fait le superviseur humain) reste hors-scope du substrat.
- Évite la dérive de l'ADR vers une mutation d'ADR-0010 (`EmitType`) sans nécessité.

**Critère de réouverture :**
- Si Phase 3 introduit un cas d'usage où il faut distinguer « escalade de premier ordre » vs « escalade répétée », ou si le filtrage par `verdict` devient insuffisamment expressif (par ex. besoin de notifier un agent superviseur applicatif spécifique), ouvrir ADR-0016 « Escalade typée et destinataires ».
- Si Phase 3 introduit un `human_supervisor` comme acteur de plein droit, le même ADR-0016 traitera la pré-allocation d'AgentId réservé.

## Alternatives considérées

| Alternative D14.a | Avantages | Inconvénients | Raison du rejet |
|-------------------|-----------|---------------|-----------------|
| Timeout par `risk_level` | Suit sémantique payload `ValidationRequest` | Triple la surface de config sans donnée pour calibrer chaque palier | Rejeté — pas de mesure justifiant la variation |
| Timeout fonction du budget de session restant | Lie naturellement les deux mécanismes | Couple ADR-0014 et ADR-0012 ; un bug dans le calcul de budget casse le timeout | Rejeté — couplage prématuré |
| Timeout encodé dans capability `cap_request_validation` | Atténuation par cap | Extension d'ADR-0005 sans cas d'usage | Reporté |
| **Valeur fixe par agent, configurable au constructeur, défaut 30 s** (retenu) | Simple, testable, configurable pour tests à délai court (50 ms) ; conforme D2 d'ADR-0013 | Valeur par défaut provisoire et non-mesurée | Retenue, avec critère de révision documenté |

| Alternative D14.b | Avantages | Inconvénients | Raison du rejet |
|-------------------|-----------|---------------|-----------------|
| Tâche Tokio séparée dans `Scheduler` | Détection centralisée | Concentre logique de politique dans `Scheduler` → déclenche D2 d'ADR-0013 (décomposition) | Rejeté — viole D2 |
| `WatchdogActor` dédié | Décomposition propre | Coût IPC sans bénéfice Phase 2 ; soulève question « qui supervise le watchdog ? » | Reporté |
| Scan périodique global | Simple, O(N)/tick | Latence grossière ; surcoût Phase 2 injustifié | Reporté |
| Pas de watchdog (poll externe) | Coût zéro substrat | Déplace le problème ; agent reste piégé tant que personne ne poll | Rejeté — ne résout pas le cas d'usage déclencheur |
| **`tokio::time::timeout` dans `run_loop`, push depuis l'agent** (retenu) | Conforme D2 ; pas d'acteur supplémentaire ; verdict émis localement | Logique de timeout dispersée dans `run_loop` plutôt que centralisée | Retenue, frontière D2 explicitée |

| Alternative D14.c | Avantages | Inconvénients | Raison du rejet |
|-------------------|-----------|---------------|-----------------|
| Re-issue automatique sur `Timeout` | Simplifie le code agent | Politique imposée sans données ; conflit avec D3 d'ADR-0013 | Rejeté |
| Backoff exponential intégré | Pattern éprouvé pour IO | Contexte « validation humaine » n'est pas un IO transient ; constantes magiques | Rejeté |
| `cap_retry` | Atténuation par cap | Extension ADR-0005 sans cas d'usage | Reporté |
| **Aucun retry automatique — `Timeout` verdict ordinaire** (retenu) | Mécanisme/politique séparés ; agent reste maître de sa logique | Force chaque agent à coder son propre retry | Retenue |

| Alternative D14.d | Avantages | Inconvénients | Raison du rejet |
|-------------------|-----------|---------------|-----------------|
| Nouvel `EmitType::Escalation` dans le log causal | Étiquette dédiée ; filtrage plus précis | Mutation d'ADR-0010 sans cas d'usage démontrant l'insuffisance du filtrage `verdict == Timeout` | Rejeté pour Phase 2 |
| Inbox `human_supervisor` (AgentId réservé) | Push direct vers superviseur | Réserve un AgentId magique ; couplage statique | Reporté |
| Capability `cap_escalate_to(target)` | Flexibilité | Demande spec de pré-allocation, sans cas d'usage | Reporté |
| **Observation via `verdict == Timeout` dans log causal existant** (retenu) | Aucun nouveau contrat ; cohérent ADR-0006 modèle A | Filtrage en O(N) sur les entrées `ValidationResponse` | Retenue, critère de réouverture ADR-0016 documenté |

## Conséquences

**Positives :**
- `ValidationVerdict::Timeout` (déclaré en `actor.rs` ligne 39 depuis L32) **est désormais effectivement émis** par un mécanisme. La dette « variant déclaré jamais produit » est éliminée.
- Un agent en `AwaitingValidation` ne peut plus rester bloqué indéfiniment. Liveness conditionnelle (cf. `spec/02-properties.md` §4.3) est partiellement adressée pour le cas A3.
- Le critère de blocage des PRs (D2 d'ADR-0013) reste protégé : aucune logique ajoutée à `Scheduler::*`. La frontière `actor::run_loop` / `Scheduler` est documentée et défendable.
- Le timeout est configurable (test à 50 ms, production à 30 s par défaut). Toute mesure future de la latence réelle de réponse permet de réviser la valeur par défaut sans toucher le mécanisme.
- Aucune mutation d'ADR-0010 (`EmitType`) : le contrat de log reste stable.

**Négatives / coûts acceptés :**
- La valeur par défaut `30_000` ms est **non-mesurée** et provisoire. Risque qu'elle reste en place par inertie au-delà de Phase 2. Mitigation : marqueur explicite dans le nom de la constante (`SESSION_DEFAULT_VALIDATION_TIMEOUT_MS`) et critère de révision documenté dans D14.a.
- La logique de timeout vit dans `run_loop` (actor.rs), pas dans un composant nommé `Supervisor`. Un lecteur futur qui cherche « où est la politique de timeout ? » devra connaître cette localisation. Mitigation : commentaire dans le code pointant vers cet ADR.
- Pas de retry automatique : chaque agent doit coder sa propre logique. Risque de duplication non-uniforme entre modules WASM. Acceptable en Phase 2 (peu de modules) ; à réviser si un pattern de retry s'impose.
- Pas d'`EmitType::Escalation` : la requête « lister tous les timeouts récents » nécessite un scan filtré des `ValidationResponse`. Coût O(N_response_entries) — acceptable Phase 2.
- L'écosystème de validation (qui répond, comment, depuis quelle UI) reste un trou. ADR-0014 borne uniquement le comportement de l'agent quand personne ne répond ; il ne spécifie pas l'expérience superviseur.

**Neutres / à surveiller :**
- Si Phase 3 introduit `AwaitingExternalIO` ou un autre état d'attente, la décision D14.b devra être généralisée. Probablement vers une fonction utilitaire `await_with_timeout` paramétrée par `(state, default_timeout)`. À envisager au moment de la généralisation, pas avant.
- Si Phase 3 fait émerger un destinataire d'escalade spécifique (par ex. acteur applicatif `policy_agent`), ouvrir ADR-0016 et migrer depuis l'observation passive vers un push explicite.
- Le scope d'ADR-0014 ne couvre pas la propagation d'erreur cross-agent (réservée à ADR-0015 par ADR-0013 §D3). Si une politique d'escalade (d) finit par produire des messages cross-agent, la frontière ADR-0014 / ADR-0015 devra être précisée — par défaut, le mécanisme de transport relève d'ADR-0015, la politique de déclenchement d'ADR-0014.
- Le critère « première logique de politique dans `Scheduler` » de D2 d'ADR-0013 n'est pas déclenché par cet ADR (toute logique vit dans `run_loop`). La prochaine politique qui *toucherait* `Scheduler::*` reste un déclencheur valide pour la décomposition.

## Références

- ADR-0013 — Architecture de supervision : canaux, états d'attente, hiérarchies (§D1 introduit `AwaitingValidation`, §D2 fixe le critère de déclenchement d'ADR-0014, §D3 réserve ADR-0015 pour la propagation d'erreur cross-agent)
- ADR-0010 — Contrat `emit` (`EmitType::ValidationRequest`, `ValidationResponse`)
- ADR-0012 — Mémoire sémantique et sessions bornées (budget de session, candidat de paramètre pour timeout)
- ADR-0005 — Design capabilities et révocation (modèle pour une éventuelle `cap_request_validation` paramétrée)
- ADR-0006 — Modèle de supervision (modèle A continu : escalade compatible avec le log causal comme canal d'audit)
- `spec/02c-primitives-agent.md` §A3 — primitive `agent_request_validation`
- `spec/02-properties.md` §4.3 — liveness conditionnelle non encore formalisée (déclencheur (3))
- `poc/runtime/src/actor.rs` ~ligne 792, ~ligne 1009, ~ligne 1021 — boucle d'attente A3 et variante `ValidationVerdict::Timeout` prévue mais non émise
- `poc/runtime/src/scheduler.rs` — surface à protéger de toute logique conditionnelle non-Acceptée
- [Armstrong 2003] *Making reliable distributed systems in the presence of software errors* — supervisor trees OTP (référence de contraste : politique unifiée par défaut au prix d'une opinion forte)
- [Klein et al. 2009, SOSP] *seL4: Formal Verification of an OS Kernel* — mécanisme/politique séparés (modèle de référence pour la décomposition à venir)

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
