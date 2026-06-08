# ADR-0015 — Propagation d'erreur cross-agent

**Date :** 2026-05-17
**Statut :** Acceptée — 2026-05-17

---

## Contexte

ADR-0013 §D3 a refusé d'implémenter un supervisor tree Erlang/OTP en Phase 2/3 et a posé le critère de réouverture du présent ADR :

> « La première fois qu'un agent crash impacte la cohésion d'un autre agent — par exemple via un appel `send_caused_by` à un enfant disparu et nécessité de propager l'échec en amont — ouvrir ADR-0015. »

L'état actuel du runtime (`poc/runtime/src/actor.rs`, `poc/runtime/src/scheduler.rs`) :

- `Scheduler::spawn_child` crée un lien causal parent → enfant (via `parent_cause` dans `parent_ids`), mais **pas** de link bidirectionnel : le parent n'est notifié d'aucun changement d'état de l'enfant.
- `run_loop` distingue déjà six chemins de terminaison anormale dans le code (échec de `process_one`, échec de `SessionResume::process_one`, chaîne `ContentStore` brisée pendant `Rollback`, `agent_terminate` depuis WASM, fermeture d'inbox, `Suspend`) — mais tous convergent vers le **même** `log_lifecycle_event(LifecycleState::Terminated)`. Aucune trace ne distingue « terminaison normale demandée » de « crash de la VM ».
- Côté parent, la seule façon actuelle de savoir qu'un enfant a disparu est un scan O(N) du log causal filtré sur `Lifecycle::Terminated` avec `agent_id == child`. C'est coûteux et la cause de la terminaison reste invisible.
- ADR-0014 §D14.d a posé le principe d'**observation passive** : les événements supervisés (timeout de validation) sont matérialisés dans le log causal sous forme d'`EmitType` existants, et le parent / superviseur les détecte par filtrage. Pas de push actif, pas de canal dédié, pas de `Receiver` à gérer côté API.
- ADR-0024 a établi le précédent d'un nouvel `EmitType` (`CompensationOpen 0x11`, `CompensationClose 0x12`) pour matérialiser un état transitoire qui doit survivre à un crash. Le pattern « un événement log = une trace auditable + détection par scan » est désormais établi dans le code (`CrashPoint` feature-gated, append idempotent, scan par CF `agent_ts`).

Le besoin concret qui ouvre ADR-0015 : un agent parent envoie `send_caused_by(child, msg)`, attend implicitement un `ActionResult` causalement lié, et l'enfant termine sans produire ce résultat. Le parent reste bloqué sur une attente qui ne se résoudra jamais, **et n'a aucun moyen de le savoir** sans scanner périodiquement le log.

### Contraintes structurantes (rappel)

1. Ne pas réintroduire un supervisor tree OTP complet (ADR-0013 §D3).
2. Ne pas coupler la politique de restart à ce mécanisme — la politique reste hors scope.
3. Ne pas muter `LifecycleState` sans justification — les états existants ont des sémantiques précises (ADR-0013 §D1).
4. Harmoniser avec ADR-0014 §D14.d : pas de duplication de mécanisme push/poll.

---

## Décisions

### D15.1 — Définition opérationnelle de l'« impact sur la cohésion »

**Décision.** Un crash d'agent **B** impacte la cohésion d'un agent **A** si et seulement si les trois conditions suivantes sont simultanément vraies :

1. Il existe dans le log causal une entrée `e_send` avec `agent_id == A`, dont le payload est un message envoyé via `Scheduler::send_caused_by(B, _, _)` (équivalent : `e_send` contient un lien causal vers B encodé dans `parent_ids` ou dans le payload du message routé).
2. B a transité dans `LifecycleState::Terminated` (entrée `Lifecycle` avec `agent_id == B`, `new_state == Terminated`) postérieurement à `e_send`.
3. Aucune entrée `ActionResult` (EmitType `0x01`) avec `agent_id == B` et `parent_ids` contenant `action_id(e_send)` n'apparaît dans le log entre `e_send` et la terminaison de B.

Cette définition est **décidable** par scan borné de la CF `agent_ts` (l'index par `(agent_id, ts_ms)`) sur la fenêtre `[ts(e_send), ts(Terminated_B)]`, donc O(k) où k = nombre d'événements de B sur l'intervalle. Elle ne dépend pas d'un état interne de l'agent parent.

**Pourquoi cette définition.** L'orientation proposée (« crash silencieux = parent ne peut pas détecter sans scanner ») mélange deux notions distinctes : (a) le fait objectif qu'un message reste sans réponse, (b) le coût de la détection. (b) est traité par D15.2 (transport). (a) est ce que cette définition capture, sans présupposer le mécanisme de détection.

Conséquences directes :

- Un crash de B **sans** `send_caused_by(B, _, _)` préalable d'aucun parent n'est **pas** un « impact sur la cohésion » au sens d'ADR-0015. C'est un événement d'orphelin local — observable via le log mais ne déclenchant pas la notification cross-agent.
- Un `ActionResult` produit par B **avant** son crash satisfait la condition (3) même si le parent ne l'a pas encore consommé : la cohérence du log prime sur l'état de consommation du parent (ADR-0010 §invariant `emit avant ack`).
- La définition ne couvre **pas** la cohésion entre pairs sans lien parent/enfant. C'est volontaire (D15.3) : la propagation latérale ouvrirait un graphe dont la fermeture n'est pas bornée a priori.

**Ce qui a été écarté.**

- « Tout `Terminated` d'enfant est un impact » : trop large. Un enfant qui termine normalement après avoir produit tous ses résultats causalement chaînés n'a pas violé la cohésion du parent.
- « Le parent déclare lui-même ce qui est un impact » : déplace le problème dans le code agent, rend la détection non-uniforme et non-vérifiable côté scheduler.

### D15.2 — Mécanisme de transport : `EmitType::AgentCrash = 0x13` dans le log causal

**Décision.** Introduire dans `poc/causal-log/src/lib.rs` :

```rust
/// ADR-0015 : terminaison anormale d'un agent — payload =
///   [cause u8 | parent_agent_id 16B | last_action_id 32B]
/// cause : 0x01 = ProcessFailed (process_one ou SessionResume a renvoyé Err),
///         0x02 = ContentStoreBroken (rollback_path a renvoyé Err),
///         0x03 = WatchdogTrap (epoch deadline dépassée, ADR-0025),
///         0x04 = HostPanic (capture par run_loop si activée).
/// AgentCrash EST l'événement terminal — aucun Lifecycle::Terminated séparé
/// n'est émis après un crash (ADR-0015 amendé 2026-05-18, D-Q-V2.2).
/// Lifecycle::Terminated est synthétisé à la lecture par os-poc-reconstruct.
AgentCrash = 0x13,
```

`parent_agent_id` est l'`agent_id` du parent direct si l'agent a été créé via `spawn_child` ; sinon `[0u8; 16]` (sentinelle « racine »). `last_action_id` est `last_action` au moment de la terminaison anormale, ou `[0u8; 32]` si aucune action n'a été émise.

La détection côté parent reste **passive**, par scan de la CF `agent_ts` filtrée sur `EmitType::AgentCrash` avec `parent_agent_id == self`. C'est exactement le pattern ADR-0014 §D14.d (filtrage du log existant).

**Pourquoi ce choix.**

- **Push minimal et persistant**. L'événement est dans le log dès l'instant du crash (avant que `run_loop` rende la main), donc auditable, content-addressé et survivant aux redémarrages. Aucun état transitoire en mémoire à gérer, aucune fuite de `Receiver` orphelin, aucune sémantique de delivery à définir.
- **Cohérent avec ADR-0024**. Le pattern « un nouvel `EmitType` pour rendre un état transitoire observable » est déjà établi pour la compensation crash-rollback. ADR-0015 réutilise le même mécanisme, ce qui simplifie l'audit et la reconstruction (ADR-0018).
- **Le `parent_agent_id` est encodé dans le payload**, pas dérivé d'un scan généalogique. Cela rend la détection côté parent O(1) à profondeur 1 et évite de devoir reconstruire l'arbre causal à chaque scan.
- **Distinguer `cause` au niveau du payload** plutôt que par des variants `EmitType` séparés évite la prolifération de codes (cf. les `0x0C–0x0F` d'ADR-0019 qui occupent déjà quatre slots pour quatre événements d'inférence). Précédent : `InferenceCancelled (0x0E)` encode `cause u8` (Rollback=0x01, Terminate=0x02) selon le même pattern.

**Ce qui a été écarté.**

- **Signal OTP-like `EXIT`** (BEAM/Erlang [Armstrong 2003, *Making reliable distributed systems in the presence of software errors*]) : requiert des links bidirectionnels persistants au niveau scheduler, c'est-à-dire une structure `HashMap<AgentId, Vec<AgentId>>` qui doit être maintenue à chaque `spawn_child`, nettoyée à chaque terminaison, et survivre aux redémarrages. Hors scope d'ADR-0013 §D3 (« pas de supervisor tree »).
- **Mach dead-name notification** ([Rashid 1986, *Threads of a New System*]) : exige le modèle capability complet avec ports comme primitive de routage ; pas en place dans le PoC actuel (les `Sender<Message>` du scheduler ne sont pas des capabilities révocables au sens d'ADR-0005). Réévaluable si le PoC migre vers un modèle Mach-like.
- **`Scheduler::watch(child_id) -> Receiver<TerminationReason>`** (one-shot notification, proposé en orientation initiale) : introduit un objet `Receiver` qu'il faut consommer ou drop, donc une sémantique de delivery (best-effort ? at-least-once ? que se passe-t-il si le parent crash avant de consommer ?). Multiplie les chemins de signalisation (log + canal in-memory) sans bénéfice mesurable tant que la latence du scan n'est pas un problème établi. Si la latence devient critique (mesure à faire en Phase 7+), un `watch` peut être ajouté **par-dessus** `EmitType::AgentCrash` sans casser le contrat — l'inverse n'est pas vrai.
- **Entrée dans le log + poll parent sans nouvel `EmitType`** : possible en s'appuyant sur `Lifecycle::Terminated` existant, mais alors la **cause** de la terminaison est invisible (l'orientation proposée le note : `run_loop` a six chemins de terminaison anormale qui convergent vers le même état). Sans distinction normal/anormal, le filtre côté parent doit appliquer la définition D15.1 à chaque `Terminated`, ce qui est faisable mais déplace la sémantique dans le consommateur — fragile.
- **Mutation de `LifecycleState`** pour ajouter `Crashed`, `Failed`, etc. : viole la contrainte 3. `LifecycleState` est un état observé à un instant ; la **cause** d'une transition est un événement distinct (cohérent avec la distinction snapshot/transition d'ADR-0006 §Modèle B).

### D15.3 — Portée : un niveau, pas d'action automatique

**Décision.** L'`EmitType::AgentCrash` ne contient que le `parent_agent_id` direct. Il n'y a **pas** de propagation transitive au niveau scheduler. Aucune action n'est déclenchée automatiquement à la suite d'un `AgentCrash` (ni suspension du parent, ni rollback, ni notification au grand-parent).

Si la politique d'un agent parent est « propage à mon propre parent », elle s'implémente côté agent : le parent observe l'`AgentCrash` de son enfant, prend une décision (rollback de session, terminaison contrôlée, log d'incident), et — s'il décide de terminer — produit son **propre** `AgentCrash` qui sera détecté à son tour par son parent. La propagation transitive est donc **émergente** et **politiquement contrôlée**, pas mécaniquement imposée.

**Pourquoi.**

- **La propagation automatique est une politique, pas un mécanisme.** Cf. Brinch Hansen, *The Nucleus of a Multiprogramming System* (1970) — la séparation mécanisme/politique impose que le noyau (ici, le scheduler) ne décide pas à la place des agents. La politique est dans le code agent ou dans un superviseur applicatif explicite.
- **Borne le coût de la détection.** À profondeur 1, le scan côté parent est O(k) sur l'intervalle pertinent. La propagation transitive automatique en O(depth × k) ouvre un coût non borné dans un graphe DAG potentiellement large (cf. ADR-0003 : les `parent_ids` peuvent être multiples — l'arbre n'est pas un arbre, c'est un DAG).
- **N'introduit pas de couplage caché.** Un grand-parent qui ignore tout d'un petit-enfant n'est pas brutalement notifié de sa terminaison. Cette propriété est testable : « pour tout `AgentCrash` de B, seul le parent direct A détecte l'événement par filtre `parent_agent_id == A` ».

**Ce qui a été écarté.**

- **Propagation transitive automatique au niveau scheduler** : remonte l'arbre causal en émettant des `AgentCrash` chaînés. Recrée de facto un supervisor tree (contrainte 1), avec en plus une sémantique de « cause originale vs cause relayée » qu'il faudrait encoder dans le payload.
- **Suspension automatique du parent à la réception** : couple le mécanisme à une politique précise. Si le parent doit être suspendu, c'est une décision qui dépend de son état interne (était-il vraiment en attente de cet enfant ? a-t-il déjà reçu un `ActionResult` d'un autre enfant qui rend la perte tolérable ?). Hors scope.
- **Rollback automatique vers le snapshot précédant le `send_caused_by`** : viole la contrainte 2 (politique de restart). Et la cohérence d'un rollback exige une analyse fine de quelles caps invalider (ADR-0007), incompatible avec un déclenchement automatique sans contexte.

### D15.4 — Relation avec ADR-0014 §D14.d : complémentarité, pas redondance

**Décision.** `EmitType::AgentCrash` et l'escalade passive d'ADR-0014 §D14.d couvrent des classes d'événements **disjointes** :

| Mécanisme | Couvre | Détecté par |
|-----------|--------|-------------|
| ADR-0014 §D14.d | `LifecycleState::AwaitingValidation` qui expire sans verdict — l'agent **vit toujours** | Filtre `EmitType::ValidationResponse` avec `verdict == Timeout (0x02)` |
| ADR-0015 D15.2 | `run_loop` termine anormalement (process_one échoue, ContentStore brisé, watchdog trap, host panic) — l'agent **disparaît** | Filtre `EmitType::AgentCrash` avec `parent_agent_id == self` |

Aucun chevauchement : un agent en `AwaitingValidation` qui crash pendant l'attente émettra **les deux** événements (un `ValidationResponse{verdict=Timeout}` injecté par `run_loop` dans la branche timeout, ou directement un `AgentCrash` si `process_one` panique). C'est la **cause de terminaison** qui détermine quel(s) événement(s) sont émis, pas un choix arbitraire.

Le pattern commun (« événement matérialisé dans le log + détection par filtre côté consommateur ») est préservé. Aucun nouveau canal, aucun nouveau type de structure d'observation, aucun nouveau code dans `Scheduler` exposé à l'API publique.

**Pourquoi pas une unification (`EmitType::Anomaly` générique)** : la distinction « l'agent vit toujours mais a dépassé un délai » vs « l'agent n'existe plus » est sémantiquement majeure pour les consommateurs (un parent peut décider de renvoyer un message dans le premier cas, jamais dans le second). Forcer un payload unique pour les deux multiplierait les codes de cause et obscurcirait l'invariant clé : `AgentCrash` est terminal et irréversible pour l'agent émetteur.

---

## Conséquences

### Modifications nécessaires

1. **`poc/causal-log/src/lib.rs`** :
   - Ajouter la variante `EmitType::AgentCrash = 0x13`.
   - Ajouter le bras `0x13 => Ok(Self::AgentCrash)` dans `impl TryFrom<u8> for EmitType`.
2. **`poc/runtime/src/actor.rs`** : `run_loop` doit émettre un `EmitType::AgentCrash` **avant** chaque transition vers `Terminated` qui correspond à un chemin anormal :
   - `process_one` retourne `Err` (ligne ~1447 et ~1630) → cause `0x01 ProcessFailed`.
   - `rollback_path` retourne `Err` (ligne ~1538) → cause `0x02 ContentStoreBroken`.
   - Watchdog epoch trap (ADR-0025) → cause `0x03 WatchdogTrap` (point d'injection à déterminer dans la branche correspondante).
   - `host panic` capturé : si une stratégie `catch_unwind` est mise en place plus tard, cause `0x04 HostPanic`. Pas de modification immédiate requise tant que le panic n'est pas capturé.
   - Les terminaisons normales (`Suspend`, inbox fermée naturellement, `agent_terminate` depuis WASM, sortie de boucle ligne ~1647 sans flag d'anomalie) n'émettent **pas** d'`AgentCrash`.
3. **`AgentState`** doit conserver l'`agent_id` du parent direct (présent au moment de `spawn_child`, à stocker comme champ `parent_agent_id: Option<AgentId>` si pas déjà accessible). À vérifier dans `Scheduler::spawn_child` et `AgentState`.
4. **`poc/os-poc-reconstruct`** (ADR-0018) : ajouter le résumé du payload `0x13` (cause humaine + parent_agent_id hex + last_action_id hex). Dette mineure, non bloquante pour ADR-0015.

### Ce qui ne change pas

- Le contrat d'`emit()` (ADR-0010) : `EmitType::AgentCrash` est émis comme tout autre `EmitType` via le pipeline standard. Pas de chemin spécial.
- `LifecycleState` : aucune nouvelle variante. `Terminated` reste émis pour les terminaisons normales (`agent_terminate`, inbox fermée, `Suspend`). **Exception post-amendement 2026-05-18** : un crash anormal (`log_agent_crash`) ne produit pas d'entrée `Lifecycle::Terminated` séparée — `AgentCrash` est suffisant et `Terminated` est synthétisé à la lecture.
- `Scheduler` : aucune nouvelle méthode publique. Pas de `watch`, pas de `link`, pas de `monitor`.
- ADR-0014 §D14.d : inchangé. Aucune duplication.
- La politique de restart : reste hors scope (futur ADR si nécessaire).

### Propriétés vérifiables après application

- **P-D15-1** (auditabilité — amendé 2026-05-18) : pour toute terminaison anormale d'agent B, il existe exactement une entrée `EmitType::AgentCrash` avec `agent_id == B` dans le log, et **aucune** entrée `Lifecycle{new_state=Terminated}` séparée n'est émise après un crash. `AgentCrash` est l'événement terminal. `Lifecycle::Terminated` est synthétisé à la lecture par `os-poc-reconstruct` (D15.2-c). Atomicité : `log_agent_crash` fixe `lifecycle=Terminated` dans le même append RocksDB (D-Q-V2.2 — voir TODO.md §D15.2-a).
- **P-D15-2** (détection bornée parent) : pour tout agent parent A, la détection des crashs de ses enfants directs est en O(k) sur la fenêtre temporelle observée, où k = nombre d'entrées d'agents enfants. Pas de dépendance à la profondeur de l'arbre.
- **P-D15-3** (non-propagation automatique) : pour tout `AgentCrash` de B, aucun autre `AgentCrash` n'est émis automatiquement par le scheduler pour A ou pour les ancêtres. Toute propagation est explicitement produite par du code agent.
- **P-D15-4** (disjointion avec D14.d) : un `ValidationResponse{verdict=Timeout}` et un `AgentCrash` ne décrivent jamais le même événement — un agent qui timeout en `AwaitingValidation` puis crash dans le cycle suivant émet successivement les deux, dans cet ordre.

### Coût

- **Espace log** : un événement supplémentaire par terminaison anormale, payload ~50 octets (1 + 16 + 32 + overhead MessagePack). Négligeable par rapport au volume `ActionResult`.
- **CPU** : un `emit` supplémentaire avant chaque `Terminated` anormal. Inférieur à la milliseconde sur cache chaud (cf. mesures ADR-0011).
- **Complexité d'implémentation** : trois sites d'émission dans `run_loop`, un champ `parent_agent_id` dans `AgentState`, une variante d'enum dans `causal-log`. Pas de nouvelle abstraction, pas de nouveau module.
- **Coût de la détection côté parent** : O(k) par scan, identique au pattern ADR-0014 §D14.d. Si la fréquence de scan devient un problème, ADR-0016 (escalade typée, réservé) peut introduire un mécanisme de notification active **par-dessus** sans casser ce contrat.

### Critère de réouverture / amendement

ADR-0015 doit être amendé si l'un des cas suivants survient :

1. La latence du scan O(k) devient mesurablement problématique (à définir : seuil > 100 ms p99 pour la détection d'un crash sur une fenêtre de 1000 événements parent). Dans ce cas : envisager `Scheduler::watch` comme couche d'optimisation **au-dessus** du log, pas en remplacement.
2. Le besoin de propagation transitive automatique émerge dans un cas concret (à documenter avec scénario reproductible). Ouvrir alors un ADR distinct pour la politique de propagation, pas pour le mécanisme.
3. La distinction des quatre causes (`0x01`–`0x04`) s'avère insuffisante (par exemple, besoin de distinguer un `ProcessFailed` venant de WASM trap vs erreur de routage). Étendre le code `cause` (1 octet = 256 valeurs, marge confortable).

---

*Format : MADR (Markdown Architecture Decision Records) — [Nygard 2011]*
*Références : [Armstrong 2003] Joe Armstrong, *Making reliable distributed systems in the presence of software errors*, PhD thesis, KTH. [Rashid 1986] Richard Rashid, *Threads of a New System*, Unix Review. [Brinch Hansen 1970] Per Brinch Hansen, *The Nucleus of a Multiprogramming System*, CACM 13(4).*
