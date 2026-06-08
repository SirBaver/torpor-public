# 02c — Primitives côté agent

## 1. Pourquoi ce document existe

### 1.1 L'asymétrie superviseur/agent est une dette de conception

`02-properties.md` formule les propriétés P1–P6 du système vu de l'extérieur. `02b-substrate_requirements.md` spécifie ce que la couche d'exécution doit fournir pour les satisfaire. Ces deux documents sont centrés sur ce que le superviseur observe et garantit.

Ce que l'agent peut faire, lire, et demander n'a pas été traité comme un cas d'usage de premier ordre. Cette asymétrie a des conséquences sur l'efficacité opérationnelle des agents : un agent qui ne peut pas introspecter son état, annuler sa propre erreur, ou signaler un doute avant d'agir est contraint à une posture plus fragile que nécessaire. La sûreté du système s'en trouve paradoxalement affectée : un agent qui ne peut pas demander validation *avant* d'agir est un agent qui échoue *après*.

### 1.2 Source de ce document

Ce document est construit à partir de deux sources :

1. **L'analyse empirique du lab (E01–E03, LESSONS.md L22–L26)** — ce que les agents font, ratent, et corrigent quand les primitives manquent.
2. **La perspective directe d'un agent LLM (Opus, 2026-05-14)** — retour sur les patterns de friction observés en conditions réelles de travail, et les primitives qui les résoudraient. Ce retour est transcrit littéralement dans `reponse.md` (2026-05-14) et indexé comme source primaire.

Ce document ne remplace pas P1–P6. Il les complète par le pendant contractuel côté agent : ce que le système doit exposer à l'agent pour que P2 (rollback), P3 (traçabilité), P4 (capabilities) soient des outils *pour* l'agent, pas seulement *sur* l'agent.

### 1.3 Statut épistémique

Les primitives A1–A4 sont **intégralement implémentées** dans `poc/runtime/src/actor.rs` (Phase 2, 2026-05-14). Chaque primitive est disponible sous forme de host function WASM. La section §3 conserve l'interface conceptuelle de haut niveau ; une sous-section "Liaison implémentation" précise la correspondance avec le code.

La gestion de session (bornée à 10K actions / 24h, résumé causal) est couverte séparément par ADR-0012 et implémentée dans le même fichier.

---

## 2. Méthode

Une primitive côté agent est retenue si :

1. **Elle corrige une asymétrie mesurée.** Il existe une friction documentée (LESSONS.md, expérience lab, ou retour agent direct) que cette primitive résoudrait.
2. **Elle est compatible avec les propriétés P1–P6.** Elle n'affaiblit aucune garantie déjà formulée.
3. **Elle est implémentable dans le modèle d'acteur WASM + capabilities.** Elle n'exige pas un substrat différent de celui identifié dans `02b`.
4. **Elle ne transforme pas le système en outil d'évasion.** Un agent ne peut pas utiliser une primitive pour court-circuiter le système de capabilities, forger des identités, ou contourner le commit barrier.

Chaque primitive est documentée avec : nom court, énoncé, motivation (friction corrigée), interface proposée, contraintes, et lien avec les propriétés.

---

## 3. Les primitives

---

### A1 — Introspection de l'état courant

**Statut :** Implémentée (Phase 2, 2026-05-14 — L29).

**Énoncé :** Un agent peut, à tout moment dans son cycle d'exécution, lire l'intégralité de son état pertinent : mémoire dans ses namespaces, capabilities actives, position dans le log causal (dernière action_id, profondeur de la chaîne), et snapshots disponibles. Cette lecture est non-bloquante et n'enregistre pas d'entrée dans le log causal.

**Motivation.** Un agent qui corrige son propre comportement à mi-parcours a besoin de savoir où il en est. Dans les conditions actuelles du lab, l'agent ne peut pas distinguer "je n'ai pas encore écrit cette valeur" de "j'ai écrit cette valeur sous une clé différente". P2.3 (divergence de schéma entre agents) est partiellement un symptôme de cette absence : l'agent ne peut pas vérifier ce qu'il a déjà écrit sans faire un appel explicite de lecture — qui lui-même peut échouer si la clé est mal nommée.

**Interface proposée :**
```
agent.introspect() → {
  namespaces: [<namespace>],
  last_action_id: <uuid>,
  causal_depth: <int>,
  active_capabilities: [<cap_id>],
  snapshots: [<snapshot_id>]
}
```

**Contraintes :**
- Lecture seule. Aucun effet de bord.
- Non enregistrée dans le log causal (ne constitue pas une action).
- Bornée en scope : l'agent voit ses namespaces et ses capabilities, pas ceux des autres agents. Compatible avec S1 (frontière de confiance).

**Liaison implémentation :**  
`agent_introspect(out_ptr: i32, out_max_len: i32) → i32` — écrit 74 bytes en mémoire WASM.  
Format binaire : `last_action_id [32B] | seq [8B u64 LE] | last_snapshot [32B] | flags [1B] | lifecycle_state [1B]`.  
Delta vs interface proposée : `namespaces`, `active_capabilities`, et `snapshots[]` (autres que le dernier) sont hors scope Phase 2 — `seq` sert de proxy pour `causal_depth`.

**Lien propriétés :** Facilite P3 (traçabilité) du côté agent ; ne change pas P3 pour le superviseur.

---

### A2 — Self-rollback borné

**Statut :** Implémentée (Phase 2, 2026-05-14 — L31).

**Énoncé :** Un agent peut annuler ses propres N dernières actions (N borné, typiquement 1–3) et revenir à l'état snapshoté avant ces actions, sans intervention du superviseur. Le self-rollback est enregistré dans le log causal comme une action de type `agent_rollback`, avec `caused_by` pointant vers la dernière action annulée.

**Motivation.** Un agent peut détecter sa propre erreur avant de la propager — typiquement en relisant sa sortie immédiate et en constatant une incohérence. Dans les conditions actuelles, l'agent n'a aucune primitive pour signaler "je viens de me tromper, annule ça". Il peut au mieux écrire une valeur corrective par-dessus, ce qui laisse une trace d'état incohérente dans le log et dans la mémoire.

L'outil est différent du rollback superviseur sur trois points :
- **Portée** : limité aux actions de l'agent lui-même, dans une fenêtre temporelle bornée.
- **Initiative** : déclenché par l'agent, pas par le superviseur.
- **Sémantique** : acte de correction, pas d'intervention externe. Le log causal enregistre explicitement que c'est l'agent qui a reconnu l'erreur.

**Interface proposée :**
```
agent.self_rollback(depth: 1..3) → {
  agent_rollback_action_id: <uuid>,
  actions_undone: [<uuid>],
  restored_state_hash: <hash>
}
```

**Contraintes :**
- Profondeur maximale bornée par configuration système (défaut : 3). Non extensible à la discrétion de l'agent.
- Interdit sur les actions dont les effets sont déjà sortis du nœud (effets externalisés post-`emit`). Après `emit`, l'effet est irrévocable — c'est la garantie du commit barrier.
- Enregistré dans le log causal. Le superviseur voit le self-rollback comme toute autre action.
- Nécessite que les actions à annuler appartiennent toutes à la même session de l'agent.

**Liaison implémentation :**  
`agent_self_rollback(depth: i32) → i32` — retourne `target_seq` (≥ 0) ou un code d'erreur : `-1` (profondeur invalide ou > MAX=3), `-2` (aucun historique), `-3` (historique insuffisant), `-4` (erreur ContentStore).  
Delta vs interface proposée : la réponse est le `target_seq` de restauration, pas un struct ; `actions_undone[]` et `restored_state_hash` sont loggés dans le CausalLog comme `EmitType::SelfRollback = 0x07` avec payload `[depth u8, target_seq u64 LE]`.

**Lien propriétés :** Complète P2 (rollback) par une initiative agent. N'affaiblit pas P2 superviseur — les deux coexistent avec des sémantiques distinctes.

---

### A3 — Canal de demande de validation

**Statut :** Implémentée (Phase 2, 2026-05-14 — L32).

**Énoncé :** Un agent peut, avant d'exécuter une action à effet irréversible, émettre une demande de validation vers son superviseur désigné. L'exécution est suspendue jusqu'à réception d'une réponse (approbation, refus, ou timeout). En cas de timeout, le comportement par défaut (continuer ou abandonner) est configurable par capability.

**Motivation.** Le modèle actuel est entièrement réactif du côté supervision : le superviseur intervient après qu'une erreur s'est produite (rollback P2) ou après qu'une anomalie est détectée (log P3). Un agent qui pressent qu'une action est risquée — parce qu'elle écrit sur un namespace partagé, parce qu'elle dépasse son mandat implicite, parce qu'elle a des effets difficiles à inverser — n'a aucune façon de signaler ce doute structurellement.

Cette asymétrie est une contrainte sur la posture de sûreté : l'OS est défensif contre l'agent plutôt que collaboratif avec lui. Dans un déploiement réel à supervision épisodique (ADR-0009 Modèle B), la posture défensive seule est insuffisante — les intervalles entre supervisions sont trop longs pour que l'interception post-hoc soit systématiquement utile.

**Interface proposée :**
```
agent.request_validation(
  action_description: <string>,
  risk_level: low | medium | high,
  timeout_seconds: <int>
) → approved | rejected | timeout
```

**Contraintes :**
- Ne peut pas être utilisée pour contourner les capabilities. Un `approved` de validation ne confère pas de permissions supplémentaires — il signale seulement que le superviseur a consenti à l'action dans les limites des capabilities existantes.
- La demande est enregistrée dans le log causal (type `validation_request`), la réponse aussi (`validation_response`). L'historique de validation est auditable.
- Timeout configurable par le superviseur, pas par l'agent. Un agent ne peut pas se fixer lui-même un timeout arbitrairement court pour court-circuiter l'attente.
- Optionnel : le superviseur peut choisir de ne pas souscrire au canal de validation — dans ce cas les demandes sont logées mais pas bloquantes (comportement "fire and forget").

**Liaison implémentation :**  
Protocole deux phases (WASM ne peut pas se suspendre mid-function) :  
1. `agent_request_validation(risk_level: i32) → i32` — logge `ValidationRequest (0x08)`, passe lifecycle → `Suspended`, retourne 0 immédiatement.  
2. `run_loop` attend `Message::ValidationResponse` dans l'inbox pendant que lifecycle == `Suspended`.  
3. `agent_get_verdict() → i32` — retourne le dernier verdict (0=Approved, 1=Rejected, 2=Timeout) ou -1 si aucun.  
Côté scheduler : `Scheduler::respond_validation(target, verdict)` envoie `Message::ValidationResponse`.  
Delta vs interface proposée : `action_description` (string) non implémenté (passage de strings WASM non trivial, différé) ; `timeout_seconds` différé à Phase 3 (non bloquant pour les cas d'usage actuels).

**Lien propriétés :** Complète P4 (capabilities) par une couche collaborative. Interagit avec ADR-0009 §supervision épisodique : la demande de validation est le mécanisme qui rend la supervision épisodique sûre sur les actions à haut risque.

---

### A4 — Cycle de vie agent explicite

**Statut :** Implémentée (Phase 2, 2026-05-14 — L30).

**Énoncé :** Le cycle de vie d'un agent est défini par cinq états et les transitions entre eux. Ces états sont visibles dans le log causal et queryables par l'agent lui-même (via A1) et par le superviseur.

```
États :
  spawned     → agent créé, en attente de sa première tâche
  active      → en cours d'exécution d'un cycle W1
  suspended   → en attente d'un événement (input, validation, ressource)
  checkpointed→ état sauvegardé, prêt pour resume ou migration
  terminated  → session close, état archivé ou purgé

Transitions :
  spawned → active         : réception d'un prompt ou d'un message
  active → suspended       : appel agent.request_validation(), attente de ressource
  active → checkpointed    : checkpoint explicite ou fin de cycle W1
  suspended → active       : réponse validation, événement attendu
  checkpointed → active    : resume avec rechargement de contexte
  checkpointed → terminated: décision superviseur ou TTL expiré
  active → terminated      : fin de tâche, fin de mandat, ou erreur non récupérable
```

**Motivation.** Le modèle actuel traite l'agent comme une Tokio task active ou inactive, sans sémantique formelle sur ce que signifie "suspendu", "checkpoint", ou "terminé". Ce manque affecte :
- La densité (P1) : un agent suspendu consomme-t-il des ressources CPU ? Une sémantique explicite permet au scheduler d'allouer différemment selon l'état.
- La récupération après incident : un agent checkpointé peut être resume sur un nœud différent. Sans sémantique de checkpoint, la migration est indéfinie.
- L'auditabilité (P3) : le log causal enregistre des actions, pas des transitions d'état. Un superviseur qui veut savoir "à quel moment cet agent est passé en mode attente" doit inférer cela des actions — ce n'est pas garanti d'être exact.

**Interface proposée :** Les transitions sont déclenchées soit par le runtime (automatiquement à la fin d'un cycle W1), soit par l'agent (`agent.checkpoint()`, `agent.terminate()`), soit par le superviseur. Chaque transition est un événement enregistré dans le log causal avec le type `lifecycle_<transition>`.

**Contraintes :**
- Un agent en état `terminated` ne peut pas se respawner lui-même. La création d'agents est une prérogative du superviseur ou d'un agent explicitement autorisé (via capability).
- Le checkpoint inclut l'état mémoire de l'agent (ses namespaces) + la dernière `action_id` + les capabilities actives. Il ne capture pas l'état interne du modèle LLM (non accessible).
- La fenêtre de contexte LLM n'est pas persistée dans le checkpoint — elle est reconstruite à partir du log causal sur le segment de session pertinent. C'est cohérent avec ADR-0009 Modèle B (log compact + reconstruction à la demande).

**Liaison implémentation :**  
`agent_checkpoint() → i32` — logge `Lifecycle::Checkpointed`, crée un snapshot ContentStore. Retourne 0.  
`agent_terminate()` — logge `Lifecycle::Terminated`, ferme proprement le module.  
`LifecycleState` : enum u8 (Spawned=0, Active=1, Suspended=2, Checkpointed=3, Terminated=4), exposé via le byte 73 de `agent_introspect`.  
`log_lifecycle_event(state)` : logge `EmitType::Lifecycle (0x05)` avec payload `[state_byte u8, seq u64 LE]`.  
Delta vs interface proposée : les transitions implicites (Active→Checkpointed à fin de cycle W1) ne sont pas encore déclenchées automatiquement par le scheduler — différé à Phase 3 (scheduler sémantique).

**Lien propriétés :** Conditionne P1 (densité) via le scheduler : un agent `suspended` ou `checkpointed` libère sa capacité d'inférence. Conditionne P3 (traçabilité) : les transitions de cycle de vie sont des événements causaux de premier ordre, pas des métadonnées implicites.

---

## 3b. Fonctions host complémentaires (Phase 2)

Ces trois fonctions ont été implémentées dans `poc/runtime/src/actor.rs` en Phase 2 pour soutenir les primitives A1–A4, mais n'ont pas été formalisées dans §3 au moment de leur ajout. Elles sont documentées ici comme contrat d'interface.

---

### `agent_session_info` — Introspection de session

**Signature :** `agent_session_info(out_ptr: i32) → i32`

Écrit 32 bytes en mémoire WASM : `session_id [16B UUID] | session_action_count [8B u64 LE] | session_started_at_ms [8B u64 LE]`.

Retourne 0 si succès, -1 si erreur. Lecture seule, non loggée dans le log causal. Complément de A1 (`agent_introspect`) centré sur la dimension session plutôt que sur la position causale.

**Relation avec ADR-0012 :** expose les bornes de session (count vs `session_max_actions`, durée vs `SESSION_DEFAULT_MAX_DURATION_MS`). Permet à un agent de détecter qu'il approche d'une frontière de session avant qu'elle soit forcée par le scheduler.

---

### `agent_add_cause` — Ajout de parent causal explicite

**Signature :** `agent_add_cause(action_id_ptr: i32) → i32`

Ajoute un `action_id` (32 bytes, SHA-256) à la liste `pending_extra_causes` de l'agent. Ces causes supplémentaires sont fusionnées dans `parent_ids[]` du prochain `LogEntry` lors du prochain `commit_barrier`.

Retourne 0 si succès, -1 si erreur de lecture mémoire. Non loggé en lui-même — l'effet apparaît dans le prochain `LogEntry` avec N > 1 parents.

**Relation avec ADR-0003 :** c'est le mécanisme d'implémentation du DAG multi-parents. Un nœud de merge (N causes explicites) se construit en appelant `agent_add_cause` pour chaque cause supplémentaire avant le `commit_barrier`.

---

### `agent_check_cap` — Vérification de capability

**Signature :** `agent_check_cap(cap_id: i64) → i32`

Vérifie si la capability `cap_id` (u64) est présente dans l'ensemble `own_caps` de l'agent. Retourne 1 si détenue, 0 sinon. Lecture seule, non loggée.

**Contrat :** ne vérifie que la présence dans le set local — pas la validité dans le store global (pas de remontée `parent_cap`). Un agent peut avoir une cap révoquée dans le store mais encore dans son `own_caps` jusqu'à la prochaine synchronisation. Pour une vérification authoritative, le scheduler doit être consulté.

**Relation avec P4 :** primitif informatif côté agent. Ne confère pas de permissions — l'autorité reste le store de capabilities vérifié par le scheduler à chaque appel host sensible.

---

## 4. Ce que ces primitives ne font pas

**Elles n'affaiblissent pas les garanties de sûreté.**

A2 (self-rollback) ne peut pas annuler des effets déjà émis via `emit`. La ligne de démarcation est le commit barrier (H-cb-correct) : avant `emit`, l'action est annulable ; après, elle ne l'est plus. C'est la même borne que pour le rollback superviseur.

A3 (canal de validation) ne confère pas de permissions. Un `approved` ne crée pas de capability — il signale le consentement du superviseur sur une action dans les limites des capabilities existantes. Le système de capabilities reste l'unique source d'autorité.

**Elles ne résolvent pas le problème de fenêtre de contexte LLM.**

A1 expose l'état *infrastructure* de l'agent (mémoire, capabilities, position causale). Elle n'expose pas l'état interne du modèle LLM — ce dernier reste une boîte noire dont le contenu dépend du contexte de conversation accumulé. La solution au problème de fenêtre de contexte est architecturale (découpage de sessions, reconstruction à partir du log) et non une primitive d'introspection.

**Elles ne substituent pas la supervision humaine.**

A3 (canal de validation) est un mécanisme d'escalade, pas de délégation. L'agent signale — le superviseur décide. La supervision asymétrique reste le modèle de référence (ADR-0006, ADR-0009).

---

## 5. Impact sur les propriétés existantes

| Propriété | Impact |
|-----------|--------|
| P1 — Densité | A4 (cycle de vie) conditionne l'allocation par le scheduler. Un agent `suspended` libère sa tranche d'inférence. Impact positif sur P1 si le ratio agents actifs/suspendus est mesuré dans T6. |
| P2 — Rollback | A2 (self-rollback) ajoute une initiative agent. P2 superviseur est inchangé. Les deux mécanismes coexistent avec des sémantiques et des autorisations distinctes. |
| P3 — Traçabilité causale | A1, A3, A4 ajoutent des entrées dans le log causal (demandes de validation, transitions de cycle de vie). La propriété P3 est renforcée — plus d'événements signifiants sont tracés. La reformulation de P3 en intégrité plutôt que latence (ADR-0009) est cohérente avec ces ajouts. |
| P4 — Capabilities | A3 (canal de validation) interagit avec P4 sans l'affaiblir. A1 permet à l'agent de lire ses propres capabilities — usage informatif, pas d'escalade de privilèges. |
| P5 — Isolation | Inchangée. Toutes les primitives A1–A4 sont confinées au scope de l'agent appelant. |
| P6 — Atomicité crash | Inchangée. A2 (self-rollback) est soumis aux mêmes garanties de durabilité que P2. |

---

## 6. Angle mort reconnu : état interne LLM

Le retour Opus (2026-05-14) note que sur les longues conversations, l'agent perd le fil de pourquoi il a pris telle décision vingt tours plus tôt. A1 expose l'état infrastructure — elle ne résout pas ce problème.

La solution structurelle est de ne pas faire dépendre la continuité de raisonnement de la fenêtre de contexte LLM. ADR-0009 (Modèle B, log compact + reconstruction à la demande) va dans ce sens : l'agent peut reconstruire son historique causal depuis le log, indépendamment de ce qui est dans sa fenêtre de contexte. Mais la reconstruction est un appel explicite, pas un état ambiant.

Ce problème est ouvert. Il n'est pas résolu par les primitives de ce document. Il est posé ici pour mémoire, en attente d'une décision sur le contrat de `emit` (ADR-0009 §Conséquences — prochain chantier de conception).

---

## Références

- `reponse.md` (2026-05-14) — retour Opus sur les frictions opérationnelles et les primitives manquantes
- `spec/02-properties.md` — P1–P6 (pendant côté superviseur)
- `spec/02b-substrate_requirements.md` — S1–S7 (exigences couche d'exécution)
- ADR-0009 — profils acteurs LLM, séparation machine/humain à `emit()`, Modèle B
- ADR-0006 — modèle de supervision (révisé par ADR-0009)
- ADR-0012 — mémoire sémantique : sessions bornées + résumé causal (complément à A4)
- `poc/runtime/src/actor.rs` — implémentation complète A1–A4 + session management
- `poc/runtime/src/lib.rs` — 12 tests A1–A4 (3 par primitive)
- `lab/LESSONS.md` §L29–L32 — comptes rendus d'implémentation A1–A4
- `lab/LESSONS.md` §L25, L26 — verbosité LLM, couche d'interposition
- `lab/experiments/E03-machine-output.md` — mesures profil T vs profil D

---

*Format inspiré de MADR (Markdown Architecture Decision Records) — [Nygard 2011] "Documenting Architecture Decisions"*
